use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "state", content = "detail")]
pub enum Status {
    Pending,
    Applying(String),   // the step in progress, e.g. "Sending files"
    Applied(String),    // what was checked, e.g. "2 files on E:, 14 settings"
    Unverified(String), // everything was sent but couldn't be read back
    Offline {
        attempt: u32,
        retry_in_secs: u64,
        reason: String,
    },
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Printer {
    pub ip: String, // IP address or hostname, exactly as entered
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub profile_id: Option<String>,
    pub status: Status,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub models: Vec<String>, // substrings matched against the printer's model, e.g. "ZD411"; empty = any
    #[serde(default)]
    pub media: Option<Media>,
    #[serde(default)]
    pub commands: Vec<String>, // extra SGD/ZPL lines to send last
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Media {
    #[serde(default)]
    pub width_in: Option<u32>, // label width in inches; none = the printer's full width
    pub media_type: MediaType,
    pub sensor: Sensor,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Gap,  // gap/notch, web sensing
    Mark, // black mark sensing
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Sensor {
    Transmissive,
    Reflective,
}

// models are substrings of the printer's model, e.g. "ZD6"; an empty list fits any printer
pub fn fits(models: &[String], model: Option<&str>) -> bool {
    models.is_empty()
        || models
            .iter()
            .any(|m| model.is_some_and(|x| x.contains(m.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_match_by_substring() {
        let zd6 = vec!["ZD611".to_string(), "ZD621".to_string()];
        assert!(fits(&zd6, Some("ZD621")));
        assert!(!fits(&zd6, Some("ZD410")));
        assert!(!fits(&zd6, None));
        assert!(fits(&["ZD".to_string()], Some("ZD410")));
        assert!(fits(&[], None));
    }

    // the GUI reads these shapes
    #[test]
    fn status_json_has_state_and_detail() {
        let json = |s: &Status| serde_json::to_value(s).unwrap();
        assert_eq!(
            json(&Status::Pending),
            serde_json::json!({"state": "Pending"})
        );
        assert_eq!(
            json(&Status::Applied("4 settings".into())),
            serde_json::json!({"state": "Applied", "detail": "4 settings"})
        );
        assert_eq!(
            json(&Status::Offline {
                attempt: 2,
                retry_in_secs: 10,
                reason: "timed out".into()
            }),
            serde_json::json!({"state": "Offline", "detail": {"attempt": 2, "retry_in_secs": 10, "reason": "timed out"}})
        );
    }

    // profiles.json is edited by hand, so the optional parts can be left out
    #[test]
    fn profiles_parse_with_optional_parts_missing() {
        let p: Vec<Profile> = serde_json::from_str(
            r#"[
                {"id": "a", "name": "commands only", "models": [], "commands": ["~JC"]},
                {"id": "b", "name": "wristbands", "models": ["ZD"],
                 "media": {"media_type": "mark", "sensor": "reflective"}}
            ]"#,
        )
        .unwrap();
        assert!(p[0].media.is_none());
        let m = p[1].media.as_ref().unwrap();
        assert!(m.width_in.is_none() && p[1].commands.is_empty());
        assert!(matches!(
            (m.media_type, m.sensor),
            (MediaType::Mark, Sensor::Reflective)
        ));
    }
}
