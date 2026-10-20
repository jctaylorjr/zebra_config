use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

const KEEP: usize = 20;

// a printer opened on the Printer or Diagnose page, kept between sessions in recent.json
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Recent {
    pub address: String, // IP or hostname, as last entered
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub ip_addr: Option<String>,
    pub last_used: u64, // seconds since 1970
}

fn file(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("no app data dir")
        .join("recent.json")
}

pub fn load(app: &AppHandle) -> Vec<Recent> {
    match fs::read_to_string(file(app)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("recent.json is invalid: {e}");
            vec![]
        }),
        Err(_) => vec![],
    }
}

pub fn save(app: &AppHandle, list: &[Recent]) -> io::Result<()> {
    let path = file(app);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, serde_json::to_string_pretty(list).unwrap())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// newest first. the same printer opened by another address, matched by serial, replaces its
// old entry
pub fn remember(list: &mut Vec<Recent>, entry: Recent) {
    list.retain(|r| {
        !(r.address.eq_ignore_ascii_case(&entry.address)
            || (entry.serial.is_some() && r.serial == entry.serial))
    });
    list.insert(0, entry);
    list.truncate(KEEP);
}

pub fn forget(list: &mut Vec<Recent>, address: &str) {
    list.retain(|r| !r.address.eq_ignore_ascii_case(address));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(address: &str, serial: Option<&str>, last_used: u64) -> Recent {
        Recent {
            address: address.into(),
            model: Some("ZD621".into()),
            serial: serial.map(String::from),
            hostname: None,
            ip_addr: None,
            last_used,
        }
    }

    #[test]
    fn newest_first_without_repeats() {
        let mut list = vec![];
        remember(&mut list, entry("192.0.2.5", Some("A1"), 1));
        remember(&mut list, entry("192.0.2.6", Some("B2"), 2));
        // the same printer again, by hostname this time
        remember(&mut list, entry("CSCA1", Some("A1"), 3));
        let addresses: Vec<&str> = list.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(addresses, ["CSCA1", "192.0.2.6"]);
        // an address typed in another case is the same address
        remember(&mut list, entry("csca1", None, 4));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].address, "csca1");
    }

    #[test]
    fn keeps_the_most_recent_twenty() {
        let mut list = vec![];
        for i in 0..25 {
            remember(
                &mut list,
                entry(&format!("192.0.2.{i}"), Some(&format!("S{i}")), i),
            );
        }
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].address, "192.0.2.24");
        assert_eq!(list.last().unwrap().address, "192.0.2.5");
    }

    #[test]
    fn forgetting_ignores_case() {
        let mut list = vec![entry("CSCA1", Some("A1"), 1), entry("192.0.2.6", None, 2)];
        forget(&mut list, "csca1");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].address, "192.0.2.6");
    }

    #[test]
    fn older_files_without_every_field_still_load() {
        let list: Vec<Recent> =
            serde_json::from_str(r#"[{"address": "192.0.2.5", "last_used": 5}]"#).unwrap();
        assert_eq!(list[0].model, None);
    }
}
