mod actions;
mod diagnose;
mod model;
mod printer;
mod profiles;
mod recent;
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
    path::Path,
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
}

#[tauri::command]
async fn add_printer(state: State<'_, AppState>, ip: String) -> Result<Printer, String> {
    // only ever talk to the IP or hostname that was entered
    let ip = ip.trim().to_string();
    if ip.is_empty() {
        return Err("enter an IP address or hostname".into());
    }
    let info = printer::query(
        &ip,
        &[
            "device.product_name",
            "device.unique_id",
            "appl.name",
            "interface.network.active.ip_addr",
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    let get = |k: &str| info.get(k).and_then(|v| v.as_str()).map(String::from);
    let p = Printer {
        ip: ip.clone(),
        model: get("device.product_name"),
        serial: get("device.unique_id"),
        firmware: get("appl.name"),
        profile_id: None,
        status: Status::Pending,
        ip_addr: get("interface.network.active.ip_addr").filter(|a| worker::usable_address(a)),
    };
    let mut printers = state.printers.lock().unwrap();
    // the same printer can be entered by hostname or by IP, so compare serials too
    if let Some(dup) = printers
        .values()
        .find(|x| x.ip.eq_ignore_ascii_case(&ip) || (p.serial.is_some() && x.serial == p.serial))
    {
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

#[tauri::command]
fn apply_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    ip: String,
    profile_id: String,
    use_folder: bool,
) -> Result<(), String> {
    let profile = state
        .profiles
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == profile_id)
        .cloned()
        .ok_or("no such profile")?;
    let folder = {
        let mut printers = state.printers.lock().unwrap();
        let p = printers.get_mut(&ip).ok_or("unknown printer")?;
        if !model::fits(&profile.models, p.model.as_deref()) {
            return Err(format!(
                "{} is not for a {}",
                profile.name,
                p.model.as_deref().unwrap_or("printer of unknown model")
            ));
        }
        p.profile_id = Some(profile_id);
        // folders hold firmware, so each model family only ever gets its own
        let settings = state.settings.lock().unwrap();
        use_folder
            .then(|| settings.folder_for(p.model.as_deref()).cloned())
            .flatten()
    };
    // re-applying cancels whatever is already running for this printer, before starting over
    let mut tasks = state.tasks.lock().unwrap();
    if let Some(old) = tasks.remove(&ip) {
        old.abort();
    }
    let handle =
        tauri::async_runtime::spawn(worker::apply_with_retry(app, ip.clone(), profile, folder));
    tasks.insert(ip, handle);
    Ok(())
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
#[derive(Serialize)]
struct Identity {
    model: Option<String>,
    serial: Option<String>,
    hostname: Option<String>, // ip.hostname on Link-OS 7.6 and later, else the friendly name
    ip_addr: Option<String>,
}

#[tauri::command]
async fn identify(ip: String) -> Result<Identity, String> {
    let v = printer::query(
        &ip,
        &[
            "device.product_name",
            "device.unique_id",
            "ip.hostname",
            "device.friendly_name",
            "interface.network.active.ip_addr",
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    let get = |k: &str| {
        v[k].as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    Ok(Identity {
        model: get("device.product_name"),
        serial: get("device.unique_id"),
        hostname: get("ip.hostname").or_else(|| get("device.friendly_name")),
        ip_addr: get("interface.network.active.ip_addr"),
    })
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
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
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
            apply_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
