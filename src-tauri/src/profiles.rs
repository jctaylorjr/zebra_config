use crate::model::{Media, MediaType, Profile, Sensor};
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager};

fn file(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("no app data dir")
        .join("profiles.json")
}

// keeps the file's order; a repeated id would be ambiguous, so only the first one counts
fn dedupe(v: Vec<Profile>) -> Vec<Profile> {
    let mut out: Vec<Profile> = vec![];
    for p in v {
        if out.iter().any(|o| o.id == p.id) {
            eprintln!(
                "profiles.json has id {} more than once; using the first",
                p.id
            );
        } else {
            out.push(p);
        }
    }
    out
}

pub fn load(app: &AppHandle) -> Vec<Profile> {
    let path = file(app);
    match fs::read_to_string(&path) {
        // never overwrite an existing file, even if it has a typo
        Ok(text) => serde_json::from_str::<Vec<Profile>>(&text)
            .map(dedupe)
            .unwrap_or_else(|e| {
                eprintln!("profiles.json is invalid: {e}");
                vec![]
            }),
        Err(_) => {
            let mut seed = vec![Profile {
                id: "feedback-on".into(),
                name: "USB mirror feedback on".into(),
                models: vec![],
                media: None,
                commands: vec![r#"! U1 setvar "usb.mirror.feedback.auto" "on""#.into()],
            }];
            seed.extend(media_defaults());
            let _ = fs::create_dir_all(path.parent().unwrap());
            let _ = fs::write(&path, serde_json::to_string_pretty(&seed).unwrap());
            seed
        }
    }
}

// the label and wristband setups for ZD printers
fn media_defaults() -> Vec<Profile> {
    let zd = |id: &str, name: &str, width_in, media_type, sensor| Profile {
        id: id.into(),
        name: name.into(),
        models: vec!["ZD".into()],
        media: Some(Media {
            width_in,
            media_type,
            sensor,
        }),
        commands: vec![],
    };
    vec![
        zd(
            "zd-2x1",
            "2x1 labels",
            Some(2),
            MediaType::Gap,
            Sensor::Transmissive,
        ),
        zd(
            "zd-4x1",
            "4x1 labels",
            Some(4),
            MediaType::Gap,
            Sensor::Transmissive,
        ),
        zd(
            "zd-wristband",
            "Wristbands",
            None,
            MediaType::Mark,
            Sensor::Reflective,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_media_profiles_round_trip_through_json() {
        let defaults = media_defaults();
        let json = serde_json::to_string(&defaults).unwrap();
        let back: Vec<Profile> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 3);
        assert!(back.iter().all(|p| p.models == ["ZD"] && p.media.is_some()));
        let widths: Vec<Option<u32>> = back
            .iter()
            .map(|p| p.media.as_ref().unwrap().width_in)
            .collect();
        assert_eq!(widths, [Some(2), Some(4), None]);
    }

    #[test]
    fn repeated_ids_keep_the_first_and_the_order() {
        let p = |id: &str, name: &str| Profile {
            id: id.into(),
            name: name.into(),
            models: vec![],
            media: None,
            commands: vec![],
        };
        let names: Vec<String> = dedupe(vec![p("b", "first b"), p("a", "a"), p("b", "second b")])
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["first b", "a"]);
    }
}
