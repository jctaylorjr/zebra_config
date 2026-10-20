mod media;
mod model;
mod printer;
mod profiles;
mod settings;
mod usb;
mod worker;

use model::{Printer, Profile, Status};
use serde::Serialize;
use std::{collections::HashMap, path::Path, sync::Mutex};
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub struct AppState {
    printers: Mutex<HashMap<String, Printer>>,
    profiles: Mutex<Vec<Profile>>, // in profiles.json order
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

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> Vec<Profile> {
    state.profiles.lock().unwrap().clone()
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
        state
            .settings
            .lock()
            .unwrap()
            .folder_for(p.model.as_deref())
            .cloned()
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            let loaded = profiles::load(app.handle());
            *app.state::<AppState>().profiles.lock().unwrap() = loaded;
            *app.state::<AppState>().settings.lock().unwrap() = settings::load(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_printer,
            list_printers,
            list_profiles,
            list_folders,
            save_folders,
            apply_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
