use crate::model::Profile;
use std::{collections::HashMap, fs, path::PathBuf};
use tauri::{AppHandle, Manager};

fn file(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("no app data dir")
        .join("profiles.json")
}

fn index(v: Vec<Profile>) -> HashMap<String, Profile> {
    v.into_iter().map(|p| (p.id.clone(), p)).collect()
}

pub fn load(app: &AppHandle) -> HashMap<String, Profile> {
    let path = file(app);
    match fs::read_to_string(&path) {
        // never overwrite an existing file, even if it has a typo
        Ok(text) => serde_json::from_str::<Vec<Profile>>(&text)
            .map(index)
            .unwrap_or_else(|e| {
                eprintln!("profiles.json is invalid: {e}");
                HashMap::new()
            }),
        Err(_) => {
            let seed = vec![Profile {
                id: "feedback-on".into(),
                name: "USB mirror feedback on".into(),
                models: vec![],
                commands: vec![r#"! U1 setvar "usb.mirror.feedback.auto" "on""#.into()],
            }];
            let _ = fs::create_dir_all(path.parent().unwrap());
            let _ = fs::write(&path, serde_json::to_string_pretty(&seed).unwrap());
            index(seed)
        }
    }
}
