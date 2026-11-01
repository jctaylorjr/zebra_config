mod actions;
mod browser;
mod clone;
mod diagnose;
mod find;
mod model;
mod printer;
mod profiles;
mod recent;
mod servicenow;
mod settings;
mod setup;
mod tray;
mod usb;
mod worker;

use model::{Printer, Profile, Status};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, atomic::AtomicBool},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[derive(Default)]
pub struct AppState {
    printers: Mutex<HashMap<String, Printer>>,
    profiles: Mutex<Vec<Profile>>,         // in profiles.json order
    profiles_error: Mutex<Option<String>>, // why profiles.json couldn't be read; saving is blocked
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    settings: Mutex<settings::Settings>,
    recent: Mutex<Vec<recent::Recent>>,
    closed_while_busy: AtomicBool, // the window was closed while printers were being configured
    servicenow: servicenow::ServiceNow, // for looking printers up by control number
}

// the profile of the browser the app reaches ServiceNow through, kept so its sign-in lasts
fn browser_profile(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("browser-edge"))
}

// where what was typed in a printer search box leads: an address as it is, or the printer a
// control number is, from ServiceNow. "find-status" events say what it's waiting on, like a
// sign-in, tagged with what was typed
#[tauri::command]
async fn find_printer(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
) -> Result<find::Found, String> {
    let profile = browser_profile(&app)?;
    let q = query.trim().to_string();
    let report = |text: &str| {
        let _ = app.emit("find-status", (&q, text));
    };
    find::find(&state.servicenow, &profile, &q, &report).await
}

// an address for what was typed: a control number is looked up, as the page may not have
async fn address(app: &AppHandle, state: &AppState, typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if find::control_number(typed).is_none() {
        return Ok(typed.to_string());
    }
    let profile = browser_profile(app)?;
    let found = find::find(&state.servicenow, &profile, typed, &|_| {}).await?;
    Ok(found.address)
}

#[tauri::command]
async fn add_printer(
    app: AppHandle,
    state: State<'_, AppState>,
    ip: String,
) -> Result<Printer, String> {
    // only ever talk to the IP or hostname that was entered, or that its control number led to
    let ip = address(&app, &state, &ip).await?;
    if ip.is_empty() {
        return Err("enter an IP address, hostname or control number".into());
    }
    let id = printer::identify(&ip).await.map_err(|e| e.to_string())?;
    let p = Printer {
        ip: ip.clone(),
        model: id.model,
        serial: id.serial,
        firmware: id.firmware,
        profile_id: None,
        status: Status::Pending,
        hostname: id.hostname,
        ip_addr: id.ip_addr,
        hidden: false,
    };
    let mut printers = state.printers.lock().unwrap();
    // the same printer can be entered by hostname or by IP, so compare serials too
    if let Some(dup) = printers
        .values_mut()
        .find(|x| x.ip.eq_ignore_ascii_case(&ip) || (p.serial.is_some() && x.serial == p.serial))
    {
        // one given a profile from its Printer page joins the list, as it is
        if dup.hidden {
            dup.hidden = false;
            return Ok(dup.clone());
        }
        return Err(format!("{ip} is already in the list as {}", dup.ip));
    }
    printers.insert(ip, p.clone());
    Ok(p)
}

#[tauri::command]
fn list_printers(state: State<'_, AppState>) -> Vec<Printer> {
    let mut printers: Vec<Printer> = state.printers.lock().unwrap().values().cloned().collect();
    printers.sort_by(|a, b| a.ip.cmp(&b.ip));
    printers
}

#[derive(Serialize)]
struct ProfileList {
    profiles: Vec<Profile>,
    error: Option<String>,
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> ProfileList {
    ProfileList {
        profiles: state.profiles.lock().unwrap().clone(),
        error: state.profiles_error.lock().unwrap().clone(),
    }
}

#[tauri::command]
fn save_profiles(
    app: AppHandle,
    state: State<'_, AppState>,
    profiles: Vec<Profile>,
) -> Result<Vec<Profile>, String> {
    if let Some(e) = state.profiles_error.lock().unwrap().as_ref() {
        return Err(format!(
            "{e}. Saving would replace it, so fix or remove the file and restart the app"
        ));
    }
    profiles::validate(&profiles)?;
    profiles::save(&app, &profiles).map_err(|e| e.to_string())?;
    *state.profiles.lock().unwrap() = profiles.clone();
    Ok(profiles)
}

// saves profiles to a file the user picks, starting on the desktop: one profile, or a list
#[tauri::command]
async fn export_profiles(app: AppHandle, profiles: Vec<Profile>) -> Result<Option<String>, String> {
    profiles::validate(&profiles)?;
    let name = match profiles.as_slice() {
        [one] => profiles::file_name(&one.name),
        _ => "Zebra profiles".to_string(),
    };
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Export profiles")
        .add_filter("Zebra profiles", &["json"])
        .set_file_name(format!("{name}.json"));
    if let Ok(desktop) = app.path().desktop_dir() {
        dialog = dialog.set_directory(desktop);
    }
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_save_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(picked) = picked else {
        return Ok(None); // cancelled
    };
    let path = picked.simplified().into_path().map_err(|e| e.to_string())?;
    let json = match profiles.as_slice() {
        [one] => serde_json::to_string_pretty(one),
        all => serde_json::to_string_pretty(all),
    }
    .map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

// reads the profiles in files the user picks; the page adds them for the user to review and save
#[tauri::command]
async fn import_profiles(app: AppHandle) -> Result<Vec<Profile>, String> {
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Import profiles")
        .add_filter("Zebra profiles", &["json"]);
    if let Ok(desktop) = app.path().desktop_dir() {
        dialog = dialog.set_directory(desktop);
    }
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_files())
        .await
        .map_err(|e| e.to_string())?;
    let mut found = vec![];
    for file in picked.unwrap_or_default() {
        let path = file.simplified().into_path().map_err(|e| e.to_string())?;
        let text =
            fs::read_to_string(&path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
        let list = profiles::parse_export(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for p in &list {
            profiles::validate(std::slice::from_ref(p))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        found.extend(list);
    }
    Ok(found)
}

// what a profile changes, with the parts that depend on a printer described
#[tauri::command]
fn preview_profile(profile: Profile) -> Result<Vec<setup::Setting>, String> {
    setup::settings(&profile, None)
}

#[tauri::command]
fn profile_template() -> Profile {
    profiles::template()
}

#[derive(Serialize)]
struct FolderView {
    #[serde(flatten)]
    folder: settings::Folder,
    summary: Option<usb::Summary>,
}

fn folder_views(folders: &[settings::Folder]) -> Vec<FolderView> {
    folders
        .iter()
        .map(|f| FolderView {
            folder: f.clone(),
            summary: f.path.as_deref().map(|p| usb::summary(Path::new(p))),
        })
        .collect()
}

#[tauri::command]
fn list_folders(state: State<'_, AppState>) -> Vec<FolderView> {
    folder_views(&state.settings.lock().unwrap().folders)
}

#[tauri::command]
fn save_folders(
    app: AppHandle,
    state: State<'_, AppState>,
    folders: Vec<settings::Folder>,
) -> Result<Vec<FolderView>, String> {
    let mut settings = state.settings.lock().unwrap();
    // only check paths that changed, so an unplugged USB doesn't block editing other folders
    for f in &folders {
        let Some(path) = &f.path else { continue };
        if settings
            .folders
            .iter()
            .any(|old| old.path.as_ref() == Some(path))
        {
            continue;
        }
        if let Some(e) = usb::summary(Path::new(path)).error {
            return Err(format!("{path}: {e}"));
        }
    }
    let new = settings::Settings {
        folders,
        theme: settings.theme,
    };
    settings::save(&app, &new).map_err(|e| e.to_string())?;
    *settings = new;
    Ok(folder_views(&settings.folders))
}

// applies a profile to the printer at `ip`. one that isn't in the Configure list, given a
// profile from its Printer page, is followed without being added there. returns the address
// it's followed under
#[tauri::command]
async fn apply_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    ip: String,
    profile_id: String,
    use_folder: bool,
) -> Result<String, String> {
    let profile = state
        .profiles
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == profile_id)
        .cloned()
        .ok_or("no such profile")?;
    let ip = listed(&state, ip.trim(), false).await?;
    if let Err(e) = start(&app, &state, &ip, profile, Some(profile_id), use_folder) {
        // one followed out of sight just now isn't kept if nothing was applied to it
        let mut printers = state.printers.lock().unwrap();
        if printers
            .get(&ip)
            .is_some_and(|p| p.hidden && p.status == Status::Pending)
        {
            printers.remove(&ip);
        }
        return Err(e);
    }
    Ok(ip)
}

// applies `profile` to the listed printer `ip`, with its model's Zebra folder if asked
fn start(
    app: &AppHandle,
    state: &AppState,
    ip: &str,
    profile: Profile,
    profile_id: Option<String>,
    use_folder: bool,
) -> Result<(), String> {
    let folder = {
        let mut printers = state.printers.lock().unwrap();
        let p = printers.get_mut(ip).ok_or("unknown printer")?;
        if !model::fits(&profile.models, p.model.as_deref()) {
            return Err(format!(
                "{} is not for a {}",
                profile.name,
                p.model.as_deref().unwrap_or("printer of unknown model")
            ));
        }
        if profile_id.is_some() {
            p.profile_id = profile_id;
        }
        // folders hold firmware, so each model family only ever gets its own
        let settings = state.settings.lock().unwrap();
        use_folder
            .then(|| settings.folder_for(p.model.as_deref()).cloned())
            .flatten()
    };
    // re-applying cancels whatever is already running for this printer, before starting over
    let mut tasks = state.tasks.lock().unwrap();
    if let Some(old) = tasks.remove(ip) {
        old.abort();
    }
    let handle = tauri::async_runtime::spawn(worker::apply_with_retry(
        app.clone(),
        ip.to_string(),
        profile,
        folder,
    ));
    tasks.insert(ip.to_string(), handle);
    Ok(())
}

// what the Clone page shows about the printer to copy from
#[tauri::command]
async fn clone_source(
    app: AppHandle,
    state: State<'_, AppState>,
    ip: String,
) -> Result<clone::Source, String> {
    let ip = address(&app, &state, &ip).await?;
    clone::read(&ip).await.map_err(|e| e.to_string())
}

// makes the printer at `to` like the one at `from`, e.g. a loaner standing in for it: lists it
// on Configure if it isn't there yet, and applies the settings picked, read from `from` now, as
// Configure applies a profile. returns where Configure lists it
#[tauri::command]
async fn clone_printer(
    app: AppHandle,
    state: State<'_, AppState>,
    from: String,
    to: String,
    pick: Vec<String>,
    use_folder: bool,
) -> Result<String, String> {
    let from = address(&app, &state, &from).await?;
    let source = clone::read(&from)
        .await
        .map_err(|e| format!("can't read {from}: {e}"))?;
    let profile = clone::profile(&source, &pick)?;
    let to = address(&app, &state, &to).await?;
    if to.is_empty() {
        return Err(
            "enter the IP address, hostname or control number of the printer to copy to".into(),
        );
    }
    let to = listed(&state, &to, true).await?;
    let serial = state
        .printers
        .lock()
        .unwrap()
        .get(&to)
        .and_then(|p| p.serial.clone());
    if serial.is_some() && serial == source.serial {
        return Err("that's the printer being copied from".into());
    }
    start(&app, &state, &to, profile, None, use_folder)?;
    Ok(to)
}

// where the app follows the printer at `address`, under another address or not, adding it if
// it isn't there yet: to the Configure list when `show` is set, and otherwise out of sight
async fn listed(state: &AppState, address: &str, show: bool) -> Result<String, String> {
    if address.is_empty() {
        return Err("enter the printer's IP address, hostname or control number".into());
    }
    let take = |p: &mut Printer| {
        p.hidden &= !show;
        p.ip.clone()
    };
    let same = |p: &&mut Printer| p.ip.eq_ignore_ascii_case(address);
    if let Some(p) = state.printers.lock().unwrap().values_mut().find(same) {
        return Ok(take(p));
    }
    let id = printer::identify(address)
        .await
        .map_err(|e| format!("can't reach {address}: {e}"))?;
    let mut printers = state.printers.lock().unwrap();
    if let Some(p) = printers
        .values_mut()
        .find(|p| id.serial.is_some() && p.serial == id.serial)
    {
        return Ok(take(p));
    }
    printers.insert(
        address.to_string(),
        Printer {
            ip: address.to_string(),
            model: id.model,
            serial: id.serial,
            firmware: id.firmware,
            profile_id: None,
            status: Status::Pending,
            hostname: id.hostname,
            ip_addr: id.ip_addr,
            hidden: !show,
        },
    );
    Ok(address.to_string())
}

// everything the printer page shows above the full settings list
#[derive(Serialize)]
struct Details {
    values: serde_json::Value, // the asked-for settings; ones the printer doesn't have are null
    status: String,
    media: bool,
    head_closed: bool,
    paused: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
}

#[tauri::command]
async fn printer_details(ip: String, keys: Vec<String>) -> Result<Details, String> {
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    // both at once, since the page asks every few seconds
    let (values, h) = tokio::join!(printer::query(&ip, &keys), printer::health(&ip));
    let (values, h) = (
        values.map_err(|e| e.to_string())?,
        h.map_err(|e| e.to_string())?,
    );
    Ok(Details {
        values,
        status: h.status,
        media: h.media,
        head_closed: h.head_closed,
        paused: h.paused,
        errors: h.errors,
        warnings: h.warnings,
    })
}

#[tauri::command]
async fn printer_settings(ip: String) -> Result<Vec<printer::Setting>, String> {
    printer::all_settings(&ip).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn printer_action(ip: String, action: actions::Action) -> Result<String, String> {
    actions::run(&ip, action).await
}

#[tauri::command]
async fn printer_calibrate(ip: String, method: actions::Calibration) -> Result<String, String> {
    actions::calibrate(&ip, method).await
}

// print width, darkness or speed, changed on the Printer page; returns what it reads back
#[tauri::command]
async fn printer_set(ip: String, key: String, value: String) -> Result<String, String> {
    actions::set(&ip, &key, &value).await
}

// sends whatever was typed, ending the line the way the printer expects
#[tauri::command]
async fn printer_raw(ip: String, command: String) -> Result<String, String> {
    let line = format!("{}\r\n", command.trim_end());
    printer::raw(&ip, line.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Preferences {
    theme: settings::Theme,
    data_dir: String, // where profiles.json, settings.json and recent.json are kept
}

#[tauri::command]
fn preferences(app: AppHandle, state: State<'_, AppState>) -> Preferences {
    Preferences {
        theme: state.settings.lock().unwrap().theme,
        data_dir: app
            .path()
            .app_data_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
    }
}

#[tauri::command]
fn set_theme(
    app: AppHandle,
    state: State<'_, AppState>,
    theme: settings::Theme,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap();
    let new = settings::Settings {
        folders: settings.folders.clone(),
        theme,
    };
    settings::save(&app, &new).map_err(|e| e.to_string())?;
    *settings = new;
    Ok(())
}

// opens the app's own data folder; the path never comes from the page
#[tauri::command]
fn open_data_folder(app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_recent(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut list = state.recent.lock().unwrap();
    list.clear();
    recent::save(&app, &list).map_err(|e| e.to_string())
}

#[tauri::command]
fn recent_printers(state: State<'_, AppState>) -> Vec<recent::Recent> {
    state.recent.lock().unwrap().clone()
}

#[tauri::command]
fn remember_printer(
    app: AppHandle,
    state: State<'_, AppState>,
    address: String,
    model: Option<String>,
    serial: Option<String>,
    hostname: Option<String>,
    ip_addr: Option<String>,
) -> Result<Vec<recent::Recent>, String> {
    let mut list = state.recent.lock().unwrap();
    recent::remember(
        &mut list,
        recent::Recent {
            address,
            model,
            serial,
            hostname,
            ip_addr,
            last_used: recent::now(),
        },
    );
    recent::save(&app, &list).map_err(|e| e.to_string())?;
    Ok(list.clone())
}

#[tauri::command]
fn forget_printer(
    app: AppHandle,
    state: State<'_, AppState>,
    address: String,
) -> Result<Vec<recent::Recent>, String> {
    let mut list = state.recent.lock().unwrap();
    recent::forget(&mut list, &address);
    recent::save(&app, &list).map_err(|e| e.to_string())?;
    Ok(list.clone())
}

// what the Diagnose page shows about a printer when it's opened
#[tauri::command]
async fn identify(ip: String) -> Result<printer::Identity, String> {
    printer::identify(&ip).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn diagnose_checks() -> Vec<diagnose::CheckInfo> {
    diagnose::catalogue()
}

// streams each log line to the page as a "diagnose-log" event, tagged with `run` so the page
// can ignore lines from a run it has moved on from
#[tauri::command]
async fn run_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    ip: String,
    checks: Vec<diagnose::Check>,
    profile_id: Option<String>,
    pc_offset_minutes: i32,
    run: u32,
) -> Result<Vec<diagnose::Finding>, String> {
    let profile = profile_id.and_then(|id| {
        state
            .profiles
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.id == id)
            .cloned()
    });
    let ctx = diagnose::Context {
        profile,
        folders: state.settings.lock().unwrap().folders.clone(),
        pc_offset_minutes,
    };
    let log = move |line: diagnose::LogLine| {
        let _ = app.emit("diagnose-log", (run, line));
    };
    Ok(diagnose::run(&ip, &checks, &ctx, &log).await)
}

#[tauri::command]
async fn apply_fix(ip: String, fix: diagnose::Fix) -> Result<String, String> {
    diagnose::apply_fix(&ip, &fix).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::default())
        .setup(|app| {
            let state = app.state::<AppState>();
            match profiles::load(app.handle()) {
                Ok(loaded) => *state.profiles.lock().unwrap() = loaded,
                Err(e) => *state.profiles_error.lock().unwrap() = Some(e),
            }
            *state.settings.lock().unwrap() = settings::load(app.handle());
            *state.recent.lock().unwrap() = recent::load(app.handle());
            tray::create(app.handle())?;
            // Edge starts out of sight and signs in to ServiceNow while the app opens, so the
            // first search by control number is as quick as the rest; it stays, with its one
            // tab, until the app closes. one that can't sign in now tries again at a search
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(profile) = browser_profile(&handle) {
                    let state = handle.state::<AppState>();
                    let _ = state.servicenow.warm_up(&profile).await;
                }
            });
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            find_printer,
            add_printer,
            list_printers,
            list_profiles,
            save_profiles,
            export_profiles,
            import_profiles,
            preview_profile,
            profile_template,
            printer_details,
            printer_settings,
            printer_action,
            printer_calibrate,
            printer_set,
            preferences,
            set_theme,
            open_data_folder,
            clear_recent,
            recent_printers,
            remember_printer,
            forget_printer,
            identify,
            diagnose_checks,
            run_diagnostics,
            apply_fix,
            printer_raw,
            list_folders,
            save_folders,
            apply_profile,
            clone_source,
            clone_printer
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
