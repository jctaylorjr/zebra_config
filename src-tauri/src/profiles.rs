use crate::model::{Interface, Media, MediaType, Network, Profile, Sensor};
use std::{fs, io, path::PathBuf};
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

// the profiles, or why profiles.json couldn't be read. the app never saves over a file it
// couldn't read, so a typo made by hand doesn't cost every profile in it
pub fn load(app: &AppHandle) -> Result<Vec<Profile>, String> {
    let path = file(app);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Vec<Profile>>(&text)
            .map(dedupe)
            .map_err(|e| format!("{} is invalid: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let seed = defaults();
            if let Err(e) = save(app, &seed) {
                eprintln!("can't write {}: {e}", path.display());
            }
            Ok(seed)
        }
        Err(e) => Err(format!("can't read {}: {e}", path.display())),
    }
}

pub fn save(app: &AppHandle, profiles: &[Profile]) -> io::Result<()> {
    let path = file(app);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, serde_json::to_string_pretty(profiles).unwrap())
}

pub fn validate(profiles: &[Profile]) -> Result<(), String> {
    for (i, p) in profiles.iter().enumerate() {
        if p.id.trim().is_empty() || p.name.trim().is_empty() {
            return Err("every profile needs a name".into());
        }
        if profiles[..i].iter().any(|o| o.id == p.id) {
            return Err(format!("two profiles have the id {}", p.id));
        }
        let name = &p.name;
        if p.darkness.is_some_and(|d| !(0.0..=30.0).contains(&d)) {
            return Err(format!("{name}: darkness must be from 0 to 30"));
        }
        if let Some(h) = &p.hostname {
            let sample = h.replace("{serial}", "X");
            if sample.is_empty()
                || !sample
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return Err(format!(
                    "{name}: a hostname can only have letters, numbers, dashes and {{serial}}"
                ));
            }
        }
        if p.media.as_ref().is_some_and(|m| m.width_in == Some(0)) {
            return Err(format!("{name}: media width must be at least 1 inch"));
        }
    }
    Ok(())
}

// an exported file holds one profile, or a list of them
pub fn parse_export(text: &str) -> Result<Vec<Profile>, String> {
    if let Ok(list) = serde_json::from_str::<Vec<Profile>>(text) {
        return Ok(list);
    }
    serde_json::from_str::<Profile>(text)
        .map(|p| vec![p])
        .map_err(|e| format!("not a profile file ({e})"))
}

// a file name from a profile's name, without the characters Windows doesn't allow
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    // nor a name ending in a dot or a space
    let clean = clean.trim().trim_end_matches('.').trim_end();
    if clean.is_empty() {
        "profile".into()
    } else {
        clean.to_string()
    }
}

// what a new profile starts with, and what a section gets when it's switched back on
pub fn template() -> Profile {
    Profile {
        id: String::new(),
        name: String::new(),
        models: vec![],
        hostname: Some("CSC{serial}".into()),
        darkness: Some(25.0),
        network: Some(Network {
            wired_dhcp: true,
            wireless_dhcp: true,
            primary: Interface::Wired,
        }),
        media: Some(Media {
            width_in: Some(2),
            media_type: MediaType::Gap,
            sensor: Sensor::Transmissive,
        }),
        commands: vec![],
    }
}

fn defaults() -> Vec<Profile> {
    media_defaults()
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
        ..template()
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

    fn named(id: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            ..template()
        }
    }

    #[test]
    fn default_media_profiles_round_trip_through_json() {
        let json = serde_json::to_string(&media_defaults()).unwrap();
        let back: Vec<Profile> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 3);
        assert!(back.iter().all(|p| p.models == ["ZD"] && p.media.is_some()));
        let widths: Vec<Option<u32>> = back
            .iter()
            .map(|p| p.media.as_ref().unwrap().width_in)
            .collect();
        assert_eq!(widths, [Some(2), Some(4), None]);
        // every printer gets the same hostname, darkness and network setup
        assert!(back.iter().all(|p| {
            p.hostname.as_deref() == Some("CSC{serial}")
                && p.darkness == Some(25.0)
                && p.network
                    .as_ref()
                    .is_some_and(|n| n.wired_dhcp && n.wireless_dhcp)
        }));
        assert!(validate(&defaults()).is_ok());
    }

    #[test]
    fn new_profiles_change_everything_by_default() {
        let t = template();
        assert!(
            t.hostname.is_some()
                && t.darkness.is_some()
                && t.network.is_some()
                && t.media.is_some()
        );
        assert!(
            validate(&[Profile {
                id: "n".into(),
                name: "n".into(),
                ..t
            }])
            .is_ok()
        );
    }

    #[test]
    fn repeated_ids_keep_the_first_and_the_order() {
        let names: Vec<String> = dedupe(vec![
            named("b", "first b"),
            named("a", "a"),
            named("b", "second b"),
        ])
        .into_iter()
        .map(|p| p.name)
        .collect();
        assert_eq!(names, ["first b", "a"]);
    }

    #[test]
    fn exported_files_hold_one_profile_or_a_list() {
        let one = serde_json::to_string(&named("a", "One")).unwrap();
        assert_eq!(parse_export(&one).unwrap()[0].name, "One");
        let both = serde_json::to_string(&vec![named("a", "One"), named("b", "Two")]).unwrap();
        assert_eq!(parse_export(&both).unwrap().len(), 2);
        assert!(
            parse_export("{\"what\": \"else\"}")
                .unwrap_err()
                .starts_with("not a profile file")
        );
        assert!(parse_export("[]").unwrap().is_empty());
    }

    #[test]
    fn file_names_are_safe_on_windows() {
        assert_eq!(file_name("2x1 labels"), "2x1 labels");
        assert_eq!(file_name("ZD/ZT: 4\"x6\"?"), "ZD-ZT- 4-x6--");
        assert_eq!(file_name("ends with dots..."), "ends with dots");
        assert_eq!(file_name("  "), "profile");
    }

    #[test]
    fn validation_catches_what_the_printer_would_reject() {
        let check = |p: Profile| validate(&[p]).err();
        assert!(check(named("a", "ok")).is_none());
        assert!(check(named("a", " ")).unwrap().contains("needs a name"));
        assert!(
            check(Profile {
                darkness: Some(31.0),
                ..named("a", "dark")
            })
            .unwrap()
            .contains("0 to 30")
        );
        assert!(
            check(Profile {
                hostname: Some("CSC {serial}".into()),
                ..named("a", "host")
            })
            .is_some()
        );
        assert!(
            check(Profile {
                hostname: Some(String::new()),
                ..named("a", "host")
            })
            .is_some()
        );
        let zero = Media {
            width_in: Some(0),
            media_type: MediaType::Gap,
            sensor: Sensor::Transmissive,
        };
        assert!(
            check(Profile {
                media: Some(zero),
                ..named("a", "media")
            })
            .is_some()
        );
        assert!(
            validate(&[named("a", "one"), named("a", "two")])
                .unwrap_err()
                .contains("id a")
        );
    }
}
