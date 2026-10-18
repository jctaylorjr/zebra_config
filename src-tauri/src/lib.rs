mod model;
mod printer;
mod profiles;
mod worker;

use model::{Printer, Profile, Status};
use std::{collections::HashMap, sync::Mutex};
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub struct AppState {
    printers: Mutex<HashMap<String, Printer>>,
    profiles: Mutex<HashMap<String, Profile>>,
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

#[tauri::command]
async fn add_printer(state: State<'_, AppState>, ip: String) -> Result<Printer, String> {
    let info = printer::query(&ip, &["device.product_name", "device.unique_id"])
        .await
        .map_err(|e| e.to_string())?;
    let get = |k: &str| info.get(k).and_then(|v| v.as_str()).map(String::from);
    let p = Printer {
        ip: ip.clone(),
        model: get("device.product_name"),
        serial: get("device.unique_id"),
        profile_id: None,
        status: Status::Pending,
    };
    state.printers.lock().unwrap().insert(ip, p.clone());
    Ok(p)
}

#[tauri::command]
fn list_printers(state: State<'_, AppState>) -> Vec<Printer> {
    state.printers.lock().unwrap().values().cloned().collect()
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> Vec<Profile> {
    state.profiles.lock().unwrap().values().cloned().collect()
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
        .get(&profile_id)
        .cloned()
        .ok_or("no such profile")?;
    {
        let mut printers = state.printers.lock().unwrap();
        let p = printers.get_mut(&ip).ok_or("unknown printer")?;
        p.profile_id = Some(profile_id);
    }
    let handle = tauri::async_runtime::spawn(worker::apply_with_retry(app, ip.clone(), profile));
    // re-applying cancels any retry loop already running for this printer
    if let Some(old) = state.tasks.lock().unwrap().insert(ip, handle) {
        old.abort();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // keep any .plugin(...) lines your template generated
        .manage(AppState::default())
        .setup(|app| {
            let loaded = profiles::load(app.handle());
            *app.state::<AppState>().profiles.lock().unwrap() = loaded;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_printer,
            list_printers,
            list_profiles,
            apply_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
