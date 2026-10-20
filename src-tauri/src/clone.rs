use crate::{
    model::{ExactWidth, Media, MediaType, Profile, Sensor},
    printer,
};
use serde::Serialize;

// print settings a loaner copies as they are, by their own names
const PLAIN: [(&str, &str); 10] = [
    ("media.speed", "Print speed (in/s)"),
    ("ezpl.print_method", "Print method"),
    ("media.printmode", "Print mode"),
    ("ezpl.tear_off", "Tear-off position"),
    ("zpl.label_top", "Label top"),
    ("zpl.left_position", "Left position"),
    ("zpl.print_orientation", "Orientation"),
    ("ezpl.head_close_action", "When the head is closed"),
    ("ezpl.power_up_action", "When turned on"),
    ("ezpl.label_length_max", "Longest label (in)"),
];

// the rest of what's read from the printer cloned from
const READ: [&str; 11] = [
    "device.product_name",
    "device.unique_id",
    "ip.hostname",
    "device.friendly_name",
    "interface.network.active.ip_addr",
    "print.tone_zpl",
    "print.tone",
    "ezpl.media_type",
    "device.sensor_select",
    "ezpl.print_width",
    "head.resolution.in_dpi",
];

// one setting a loaner can copy, as the printer cloned from has it
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Setting {
    pub id: &'static str,
    pub label: &'static str,
    pub value: String,
}

// the printer cloned from, as read
#[derive(Serialize, Clone, Debug)]
pub struct Source {
    pub model: Option<String>,
    pub serial: Option<String>,
    pub hostname: Option<String>,
    pub ip_addr: Option<String>,
    pub settings: Vec<Setting>, // only what it has
    #[serde(skip)]
    values: serde_json::Value,
}

pub async fn read(ip: &str) -> anyhow::Result<Source> {
    let keys: Vec<&str> = READ
        .iter()
        .copied()
        .chain(PLAIN.iter().map(|(key, _)| *key))
        .collect();
    Ok(source(printer::query(ip, &keys).await?))
}

// a value the printer has; settings it doesn't have come back as null, or "?" on some printers
fn value(v: &serde_json::Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "?")
        .map(String::from)
}

fn source(values: serde_json::Value) -> Source {
    let get = |key: &str| value(&values, key);
    let hostname = get("ip.hostname").or_else(|| get("device.friendly_name"));
    let mut settings = vec![];
    let mut add = |id, label, value: Option<String>| {
        if let Some(value) = value {
            settings.push(Setting { id, label, value });
        }
    };
    add("hostname", "Hostname", hostname.clone());
    add(
        "darkness",
        "Darkness (0-30)",
        darkness(&values).map(|d| d.to_string()),
    );
    let width = media(&values).and_then(|m| m.exact_width);
    add(
        "media",
        "Media, sensor and print width",
        width.map(|w| {
            let (kind, sensor) = (get("ezpl.media_type"), get("device.sensor_select"));
            format!(
                "{}, {} sensor, {} dots at {} dpi",
                kind.unwrap_or_default(),
                sensor.unwrap_or_default(),
                w.dots,
                w.dpi
            )
        }),
    );
    for (key, label) in PLAIN {
        add(key, label, get(key));
    }
    Source {
        model: get("device.product_name"),
        serial: get("device.unique_id"),
        hostname,
        ip_addr: get("interface.network.active.ip_addr").filter(|a| a != "0.0.0.0"),
        settings,
        values,
    }
}

// on the 0-30 scale; print.tone follows the printer's language, which is ZPL on these
fn darkness(v: &serde_json::Value) -> Option<f64> {
    value(v, "print.tone_zpl")
        .or_else(|| value(v, "print.tone"))?
        .parse()
        .ok()
}

fn media(v: &serde_json::Value) -> Option<Media> {
    let media_type = match value(v, "ezpl.media_type")?.as_str() {
        "gap/notch" => MediaType::Gap,
        "mark" => MediaType::Mark,
        "continuous" => MediaType::Continuous,
        _ => return None,
    };
    let sensor = match value(v, "device.sensor_select")?.as_str() {
        "transmissive" => Sensor::Transmissive,
        "reflective" => Sensor::Reflective,
        "auto" => Sensor::Auto,
        _ => return None,
    };
    let dots = value(v, "ezpl.print_width")?.parse().ok()?;
    let dpi = value(v, "head.resolution.in_dpi")?.parse().ok()?;
    Some(Media {
        width_in: None,
        exact_width: Some(ExactWidth { dots, dpi }),
        media_type,
        sensor,
    })
}

// a profile that makes a printer like the one `from` was read from, with the settings picked by
// id. the hostname is copied as it is, so a loaner answers to the name of the printer it stands
// in for, even though that name holds the other printer's serial
pub fn profile(from: &Source, pick: &[String]) -> Result<Profile, String> {
    let picked =
        |id: &str| pick.iter().any(|p| p == id) && from.settings.iter().any(|s| s.id == id);
    let mut commands = vec![];
    for (key, _) in PLAIN {
        // a quote would end the value early
        if let Some(v) = value(&from.values, key).filter(|v| picked(key) && !v.contains('"')) {
            commands.push(format!(r#"! U1 setvar "{key}" "{v}""#));
        }
    }
    let name = from
        .hostname
        .as_deref()
        .or(from.ip_addr.as_deref())
        .unwrap_or("another printer");
    let p = Profile {
        id: "clone".into(),
        name: format!("Clone of {name}"),
        models: vec![],
        hostname: from.hostname.clone().filter(|_| picked("hostname")),
        darkness: darkness(&from.values).filter(|_| picked("darkness")),
        network: None,
        media: media(&from.values).filter(|_| picked("media")),
        commands,
    };
    if p.hostname.is_none() && p.darkness.is_none() && p.media.is_none() && p.commands.is_empty() {
        return Err("choose at least one setting to copy".into());
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup;

    // what a ZD410 on Link-OS 6.4 reported
    fn zd410() -> Source {
        source(serde_json::json!({
            "device.product_name": "ZD410",
            "device.unique_id": "50J182801960",
            "ip.hostname": null,
            "device.friendly_name": "CSC722858",
            "interface.network.active.ip_addr": "172.17.255.41",
            "print.tone_zpl": "25.0",
            "print.tone": "25.0",
            "ezpl.media_type": "gap/notch",
            "device.sensor_select": "transmissive",
            "ezpl.print_width": "406",
            "head.resolution.in_dpi": "203",
            "media.speed": "6.0",
            "ezpl.print_method": "direct thermal",
            "media.printmode": "tear off",
            "ezpl.tear_off": "0",
            "zpl.label_top": "0",
            "zpl.left_position": "0",
            "zpl.print_orientation": "nor",
            "ezpl.head_close_action": "feed",
            "ezpl.power_up_action": "no motion",
            "ezpl.label_length_max": "39",
        }))
    }

    fn ids(s: &Source) -> Vec<&str> {
        s.settings.iter().map(|s| s.id).collect()
    }

    #[test]
    fn lists_what_the_printer_has() {
        let s = zd410();
        assert_eq!(s.hostname.as_deref(), Some("CSC722858"));
        assert_eq!(&ids(&s)[..3], ["hostname", "darkness", "media"]);
        assert_eq!(s.settings.len(), 13);
        assert_eq!(
            s.settings[2].value,
            "gap/notch, transmissive sensor, 406 dots at 203 dpi"
        );
        // a printer without a setting doesn't offer it
        let mut v = s.values.clone();
        v["zpl.label_top"] = serde_json::Value::Null;
        v["ezpl.tear_off"] = "?".into();
        let fewer = source(v);
        assert!(!ids(&fewer).contains(&"zpl.label_top") && !ids(&fewer).contains(&"ezpl.tear_off"));
    }

    #[test]
    fn copies_the_hostname_as_it_is_onto_a_printer_with_another_serial() {
        let all: Vec<String> = zd410().settings.iter().map(|s| s.id.to_string()).collect();
        let p = profile(&zd410(), &all).unwrap();
        assert_eq!(p.name, "Clone of CSC722858");
        assert_eq!(p.hostname.as_deref(), Some("CSC722858"));
        assert_eq!(p.darkness, Some(25.0));
        assert!(
            p.commands
                .contains(&r#"! U1 setvar "media.speed" "6.0""#.to_string())
        );
        assert!(
            p.commands
                .contains(&r#"! U1 setvar "media.printmode" "tear off""#.to_string())
        );
        assert_eq!(p.commands.len(), 10);

        let loaner = setup::Target {
            serial: "50J999999999".into(),
            dpi: 203,
            max_dots: 448,
            ip_hostname: false,
            tone_zpl: true,
        };
        let s = setup::settings(&p, Some(&loaner)).unwrap();
        let get = |key: &str| s.iter().find(|s| s.key == key).map(|s| s.value.as_str());
        assert_eq!(get("device.friendly_name"), Some("CSC722858"));
        assert_eq!(get("ezpl.print_width"), Some("406"));
        // the same width on a 300 dpi loaner is more dots
        let fine = setup::Target {
            dpi: 300,
            max_dots: 1280,
            ..loaner
        };
        let s = setup::settings(&p, Some(&fine)).unwrap();
        assert!(
            s.iter()
                .any(|s| s.key == "ezpl.print_width" && s.value == "600")
        );
    }

    // reads a real printer, so it only runs when asked. prints a profile with all of its settings,
    // for worker's apply_to_printer test to apply; ZEBRA_CLONE_NAME copies it under another name:
    // ZEBRA_CLONE_FROM=192.168.1.50 ZEBRA_CLONE_NAME=TEST1 cargo test clone_profile -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn clone_profile() {
        let from = std::env::var("ZEBRA_CLONE_FROM").expect("set ZEBRA_CLONE_FROM");
        let source = read(&from).await.unwrap();
        for s in &source.settings {
            println!("{:<32} {}", s.label, s.value);
        }
        let all: Vec<String> = source.settings.iter().map(|s| s.id.to_string()).collect();
        let mut p = profile(&source, &all).unwrap();
        if let Ok(name) = std::env::var("ZEBRA_CLONE_NAME") {
            p.hostname = Some(name);
        }
        println!("PROFILE {}", serde_json::to_string(&p).unwrap());
    }

    #[test]
    fn copies_only_what_was_picked() {
        let p = profile(&zd410(), &["darkness".into(), "media.speed".into()]).unwrap();
        assert_eq!(p.hostname, None);
        assert!(p.media.is_none());
        assert_eq!(p.darkness, Some(25.0));
        assert_eq!(p.commands, [r#"! U1 setvar "media.speed" "6.0""#]);
        // nothing to copy
        assert!(profile(&zd410(), &[]).is_err());
        assert!(profile(&zd410(), &["wlan.essid".into()]).is_err());
    }
}
