use crate::model;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};
use tauri::{AppHandle, Manager};

// changed from the GUI, unlike profiles.json which is edited by hand
#[derive(Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_folders")]
    pub folders: Vec<Folder>,
}

// a USB mirror "Zebra" folder and the printers it's for
#[derive(Serialize, Deserialize, Clone)]
pub struct Folder {
    pub models: Vec<String>, // substrings matched against the printer's model; the first match wins
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub firmware_version: Option<String>, // appl.name its firmware installs; printers already on it are skipped
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            folders: default_folders(),
        }
    }
}

impl Settings {
    pub fn folder_for(&self, printer_model: Option<&str>) -> Option<&Folder> {
        self.folders.iter().find(|f| {
            // an empty model list never matches, since folders can hold firmware
            f.path.is_some() && !f.models.is_empty() && model::fits(&f.models, printer_model)
        })
    }
}

fn default_folders() -> Vec<Folder> {
    [vec!["ZD611", "ZD621"], vec!["ZT"]]
        .into_iter()
        .map(|models| Folder {
            models: models.into_iter().map(String::from).collect(),
            path: None,
            firmware_version: None,
        })
        .collect()
}

fn file(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("no app data dir")
        .join("settings.json")
}

pub fn load(app: &AppHandle) -> Settings {
    match fs::read_to_string(file(app)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("settings.json is invalid: {e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save(app: &AppHandle, s: &Settings) -> io::Result<()> {
    let path = file(app);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, serde_json::to_string_pretty(s).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(models: &[&str], path: Option<&str>) -> Folder {
        Folder {
            models: models.iter().map(|m| m.to_string()).collect(),
            path: path.map(String::from),
            firmware_version: None,
        }
    }

    #[test]
    fn each_model_family_gets_only_its_own_folder() {
        let s = Settings {
            folders: vec![
                folder(&["ZD611", "ZD621"], Some("/zd")),
                folder(&["ZT"], None),
                folder(&[], Some("/any")),
                folder(&["ZD"], Some("/zd-other")),
            ],
        };
        let path = |model| s.folder_for(model).and_then(|f| f.path.as_deref());
        assert_eq!(path(Some("ZD621")), Some("/zd"));
        // ZT has no folder chosen, and an empty model list never matches
        assert_eq!(path(Some("ZT411")), None);
        assert_eq!(path(None), None);
        // the first match wins, so a ZD410 falls through to the general ZD folder
        assert_eq!(path(Some("ZD410")), Some("/zd-other"));
    }

    #[test]
    fn missing_settings_get_the_default_folders() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.folders.len(), 2);
        assert!(s.folders.iter().all(|f| f.path.is_none()));
    }
}
