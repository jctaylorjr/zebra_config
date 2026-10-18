mod model;
mod printer;

use model::{Printer, Status};
use std::{collections::HashMap, sync::Mutex};
use tauri::State;

#[derive(Default)]
struct AppState {
    printers: Mutex<HashMap<String, Printer>>,
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
        status: Status::Pending,
    };
    state.printers.lock().unwrap().insert(ip, p.clone());
    Ok(p)
}

#[tauri::command]
fn list_printers(state: State<'_, AppState>) -> Vec<Printer> {
    state.printers.lock().unwrap().values().cloned().collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // keep any .plugin(...) lines your template generated
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![add_printer, list_printers])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
