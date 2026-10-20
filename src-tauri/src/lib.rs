mod actions;
mod diagnose;
mod model;
mod printer;
mod profiles;
mod settings;
mod setup;
mod usb;
mod worker;

use model::{Printer, Profile, Status};
use serde::Serialize;
use std::{collections::HashMap, path::Path, sync::Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct AppState {
    printers: Mutex<HashMap<String, Printer>>,
    profiles: Mutex<Vec<Profile>>,         // in profiles.json order
    profiles_error: Mutex<Option<String>>, // why profiles.json couldn't be read; saving is blocked
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    settings: Mutex<settings::Settings>,
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
        &["device.product_name", "device.unique_id", "appl.name"],
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
    let new = settings::Settings { folders };
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
    let values = printer::query(&ip, &keys)
        .await
        .map_err(|e| e.to_string())?;
    let h = printer::health(&ip).await.map_err(|e| e.to_string())?;
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
        .manage(AppState::default())
        .setup(|app| {
            let state = app.state::<AppState>();
            match profiles::load(app.handle()) {
                Ok(loaded) => *state.profiles.lock().unwrap() = loaded,
                Err(e) => *state.profiles_error.lock().unwrap() = Some(e),
            }
            *state.settings.lock().unwrap() = settings::load(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_printer,
            list_printers,
            list_profiles,
            save_profiles,
            preview_profile,
            profile_template,
            printer_details,
            printer_settings,
            printer_action,
            printer_calibrate,
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
