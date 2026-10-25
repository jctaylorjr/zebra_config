use crate::model::{Interface, Media, MediaType, Profile, Sensor};
use serde::Serialize;

// the printer a profile is being applied to
pub struct Target {
    pub serial: String,
    pub dpi: u32,
    pub max_dots: u32,     // the widest ezpl.print_width it accepts
    pub ip_hostname: bool, // has ip.hostname (Link-OS 7.6 and later)
    pub tone_zpl: bool,    // has print.tone_zpl, darkness on the 0-30 scale whatever the language
}

// one setting a profile changes, as listed in the profile manager
#[derive(Serialize, Debug, PartialEq)]
pub struct Setting {
    pub label: &'static str,
    pub key: String,
    pub value: String,
}

// print width in dots: whole inches at the printer's resolution, never more than its nominal
// width. a ZD410 accepts 448 dots but is a 2 inch printer, so its widest is 2 x 203 = 406. a
// width copied from another printer is kept the same size at this one's resolution instead
pub fn width(m: &Media, dpi: u32, max_dots: u32) -> Result<u32, String> {
    if let Some(w) = m.exact_width {
        let from = w.dpi.max(1);
        let dots = (w.dots * dpi + from / 2) / from;
        if dots == 0 || dots > max_dots {
            return Err(format!(
                "{} dots at {} dpi is wider than this printer's {max_dots} dots at {dpi} dpi",
                w.dots, w.dpi
            ));
        }
        return Ok(dots);
    }
    let max_in = max_dots / dpi.max(1);
    let width_in = m.width_in.unwrap_or(max_in);
    if width_in == 0 || width_in > max_in {
        return Err(format!(
            "{width_in} inch media is wider than this printer's {max_in} inches"
        ));
    }
    Ok(width_in * dpi)
}

// every setting a profile changes, in the order they're sent. without a printer, values that
// depend on one are described instead, e.g. "CSC{serial}" or "2 in x printer dpi"
pub fn settings(p: &Profile, target: Option<&Target>) -> Result<Vec<Setting>, String> {
    let mut out = vec![];
    let mut set = |label, key: &str, value: String| {
        out.push(Setting {
            label,
            key: key.into(),
            value,
        })
    };

    if let Some(name) = &p.hostname {
        let name = match target {
            Some(t) => name.replace("{serial}", &t.serial),
            None => name.clone(),
        };
        // before Link-OS 7.6 the friendly name is the hostname. from 7.6 the hostname is
        // ip.hostname, and the friendly name only names the printer elsewhere, e.g. Bluetooth
        if target.is_none_or(|t| t.ip_hostname) {
            set(
                "Hostname (Link-OS 7.6 and later)",
                "ip.hostname",
                name.clone(),
            );
        }
        set(
            "Friendly name (hostname before Link-OS 7.6)",
            "device.friendly_name",
            name,
        );
        // each interface's DHCP host name follows the hostname once option 12 is on
        let on = || "on".to_string();
        set(
            "Send hostname over wired DHCP",
            "internal_wired.ip.dhcp.option12",
            on(),
        );
        set(
            "Send hostname over wireless DHCP",
            "wlan.ip.dhcp.option12",
            on(),
        );
    }
    if let Some(d) = p.darkness {
        // print.tone follows the printer's language, so a mobile printer set to CPCL reads it on
        // another scale; print.tone_zpl is always 0 to 30. read back with one decimal, e.g. "25.0"
        let key = match target {
            Some(t) if !t.tone_zpl => "print.tone",
            _ => "print.tone_zpl",
        };
        set("Darkness", key, format!("{d:.1}"));
    }
    if let Some(n) = &p.network {
        if n.wired_dhcp {
            set("Wired network", "internal_wired.ip.protocol", "dhcp".into());
        }
        if n.wireless_dhcp {
            set("Wireless network", "wlan.ip.protocol", "dhcp".into());
        }
        let primary = match n.primary {
            Interface::Wired => "1",
            Interface::Wireless => "2",
        };
        set(
            "Preferred network (1 wired, 2 wireless)",
            "ip.primary_network",
            primary.into(),
        );
    }
    if let Some(m) = &p.media {
        // continuous media has no gaps or marks to sense
        let (media_type, sense_mode) = match m.media_type {
            MediaType::Gap => ("gap/notch", Some("gap")),
            MediaType::Mark => ("mark", Some("bar")),
            MediaType::Continuous => ("continuous", None),
        };
        let sensor = match m.sensor {
            Sensor::Transmissive => "transmissive",
            Sensor::Reflective => "reflective",
            Sensor::Auto => "auto",
        };
        let width = match target {
            Some(t) => width(m, t.dpi, t.max_dots)?.to_string(),
            None => match (m.exact_width, m.width_in) {
                (Some(w), _) => format!(
                    "{} dots at {} dpi, the same size at printer dpi",
                    w.dots, w.dpi
                ),
                (None, Some(w)) => format!("{w} in x printer dpi"),
                (None, None) => "printer's full width in whole inches".into(),
            },
        };
        set("Media type", "ezpl.media_type", media_type.into());
        if let Some(sense_mode) = sense_mode {
            set("Sense mode", "media.sense_mode", sense_mode.into());
        }
        set("Sensor", "device.sensor_select", sensor.into());
        set("Print width (dots)", "ezpl.print_width", width);
        if let Some(dots) = m.max_length {
            let inches = match target {
                Some(t) => max_length_in(dots, t.dpi).to_string(),
                None => format!("{dots} dots / printer dpi, in whole inches"),
            };
            set("Maximum label length (in)", "ezpl.label_length_max", inches);
        }
    }
    Ok(out)
}

// a maximum label length in dots, as the printer keeps it: whole inches, rounded, so ^ML6000
// on a 203 dpi printer reads back as 30
pub fn max_length_in(dots: u32, dpi: u32) -> u32 {
    let dpi = dpi.max(1);
    ((dots + dpi / 2) / dpi).max(1)
}

// the lines to send: the settings, then a profile's extra commands
pub fn commands(settings: &[Setting], extra: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = settings
        .iter()
        .map(|s| format!(r#"! U1 setvar "{}" "{}""#, s.key, s.value))
        .collect();
    lines.extend(extra.iter().cloned());
    lines
}

// one label of left-aligned text, scaled so it's the same size at any dpi and fits a 1 inch label
pub fn test_label(lines: &[String], dpi: u32) -> String {
    let dots = |at_203: u32| at_203 * dpi / 203;
    let mut zpl = String::from("^XA^CI28");
    let mut y = dots(20);
    for (i, line) in lines.iter().enumerate() {
        let size = dots(if i == 0 { 32 } else { 24 });
        let text: String = line.chars().filter(|c| !matches!(c, '^' | '~')).collect();
        zpl += &format!("^FO{},{y}^A0N,{size},{size}^FD{text}^FS", dots(20));
        y += size + dots(6);
    }
    zpl + "^XZ"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Network;

    fn profile(media: Option<Media>) -> Profile {
        Profile {
            id: "p".into(),
            name: "p".into(),
            models: vec![],
            hostname: None,
            darkness: None,
            network: None,
            media,
            commands: vec![],
        }
    }

    fn labels(width_in: Option<u32>) -> Profile {
        profile(Some(Media {
            width_in,
            exact_width: None,
            max_length: None,
            media_type: MediaType::Gap,
            sensor: Sensor::Transmissive,
        }))
    }

    fn target(dpi: u32, max_dots: u32) -> Target {
        Target {
            serial: "ABC123456789".into(),
            dpi,
            max_dots,
            ip_hostname: false,
            tone_zpl: true,
        }
    }

    fn lines(p: &Profile, t: &Target) -> Vec<String> {
        commands(&settings(p, Some(t)).unwrap(), &p.commands)
    }

    fn print_width(p: &Profile, t: &Target) -> Result<String, String> {
        let s = settings(p, Some(t))?;
        Ok(s.into_iter()
            .find(|s| s.key == "ezpl.print_width")
            .unwrap()
            .value)
    }

    #[test]
    fn widths_are_whole_inches_capped_at_the_printers_size() {
        // ZD410: 2 inch, 203 dpi, accepts up to 448 dots
        assert_eq!(
            print_width(&labels(Some(2)), &target(203, 448)).unwrap(),
            "406"
        );
        assert_eq!(
            print_width(&labels(None), &target(203, 448)).unwrap(),
            "406"
        );
        assert!(print_width(&labels(Some(4)), &target(203, 448)).is_err());
        // ZD621: 4 inch at 203 and 300 dpi
        assert_eq!(
            print_width(&labels(Some(4)), &target(203, 832)).unwrap(),
            "812"
        );
        assert_eq!(
            print_width(&labels(None), &target(203, 832)).unwrap(),
            "812"
        );
        assert_eq!(
            print_width(&labels(Some(2)), &target(300, 1280)).unwrap(),
            "600"
        );
        assert_eq!(
            print_width(&labels(None), &target(300, 1280)).unwrap(),
            "1200"
        );
    }

    #[test]
    fn wristbands_use_mark_sensing() {
        let p = profile(Some(Media {
            width_in: None,
            exact_width: None,
            max_length: None,
            media_type: MediaType::Mark,
            sensor: Sensor::Reflective,
        }));
        let lines = lines(&p, &target(203, 448));
        assert_eq!(lines[0], r#"! U1 setvar "ezpl.media_type" "mark""#);
        assert_eq!(lines[1], r#"! U1 setvar "media.sense_mode" "bar""#);
        assert_eq!(
            lines[2],
            r#"! U1 setvar "device.sensor_select" "reflective""#
        );
    }

    #[test]
    fn wristbands_set_a_long_maximum_label_length_in_whole_inches() {
        let p = profile(Some(Media {
            width_in: None,
            exact_width: None,
            max_length: Some(6000),
            media_type: MediaType::Mark,
            sensor: Sensor::Reflective,
        }));
        // what a ZD410 reads back after ^ML6000
        assert_eq!(
            lines(&p, &target(203, 448)).last().unwrap(),
            r#"! U1 setvar "ezpl.label_length_max" "30""#
        );
        assert_eq!(
            lines(&p, &target(300, 1280)).last().unwrap(),
            r#"! U1 setvar "ezpl.label_length_max" "20""#
        );
        let preview = settings(&p, None).unwrap();
        assert_eq!(
            preview.last().unwrap().value,
            "6000 dots / printer dpi, in whole inches"
        );
        assert_eq!(max_length_in(1, 203), 1);
    }

    #[test]
    fn printer_defaults_fill_in_the_serial() {
        let mut p = profile(None);
        p.hostname = Some("CSC{serial}".into());
        p.darkness = Some(25.0);
        p.network = Some(Network {
            wired_dhcp: true,
            wireless_dhcp: true,
            primary: Interface::Wired,
        });
        p.commands = vec!["~JC".into()];
        let lines = lines(&p, &target(203, 448));
        // a printer before Link-OS 7.6 doesn't have ip.hostname
        assert_eq!(
            lines,
            [
                r#"! U1 setvar "device.friendly_name" "CSCABC123456789""#,
                r#"! U1 setvar "internal_wired.ip.dhcp.option12" "on""#,
                r#"! U1 setvar "wlan.ip.dhcp.option12" "on""#,
                r#"! U1 setvar "print.tone_zpl" "25.0""#,
                r#"! U1 setvar "internal_wired.ip.protocol" "dhcp""#,
                r#"! U1 setvar "wlan.ip.protocol" "dhcp""#,
                r#"! U1 setvar "ip.primary_network" "1""#,
                "~JC",
            ]
        );
    }

    #[test]
    fn hostname_also_goes_to_ip_hostname_on_newer_firmware() {
        let mut p = profile(None);
        p.hostname = Some("CSC{serial}".into());
        let newer = Target {
            ip_hostname: true,
            ..target(203, 832)
        };
        let lines = lines(&p, &newer);
        assert_eq!(lines[0], r#"! U1 setvar "ip.hostname" "CSCABC123456789""#);
        assert_eq!(
            lines[1],
            r#"! U1 setvar "device.friendly_name" "CSCABC123456789""#
        );
        // the preview can't know the firmware, so it lists both
        let keys: Vec<String> = settings(&p, None)
            .unwrap()
            .into_iter()
            .map(|s| s.key)
            .collect();
        assert!(
            keys.contains(&"ip.hostname".into()) && keys.contains(&"device.friendly_name".into())
        );
    }

    #[test]
    fn previews_describe_what_depends_on_the_printer() {
        let mut p = labels(Some(2));
        p.hostname = Some("CSC{serial}".into());
        p.network = Some(Network {
            wired_dhcp: false,
            wireless_dhcp: true,
            primary: Interface::Wireless,
        });
        let s = settings(&p, None).unwrap();
        let value = |key: &str| s.iter().find(|s| s.key == key).map(|s| s.value.as_str());
        assert_eq!(value("device.friendly_name"), Some("CSC{serial}"));
        assert_eq!(value("ezpl.print_width"), Some("2 in x printer dpi"));
        // an unchecked interface isn't touched
        assert_eq!(value("internal_wired.ip.protocol"), None);
        assert_eq!(value("ip.primary_network"), Some("2"));
    }

    #[test]
    fn darkness_uses_the_zpl_scale_when_the_printer_has_it() {
        let mut p = profile(None);
        p.darkness = Some(20.0);
        let older = Target {
            tone_zpl: false,
            ..target(203, 448)
        };
        assert_eq!(lines(&p, &older), [r#"! U1 setvar "print.tone" "20.0""#]);
        assert_eq!(
            lines(&p, &target(203, 448)),
            [r#"! U1 setvar "print.tone_zpl" "20.0""#]
        );
    }

    #[test]
    fn continuous_media_sets_no_sense_mode() {
        let p = profile(Some(Media {
            width_in: Some(3),
            exact_width: None,
            max_length: None,
            media_type: MediaType::Continuous,
            sensor: Sensor::Transmissive,
        }));
        let keys: Vec<String> = settings(&p, Some(&target(203, 640)))
            .unwrap()
            .into_iter()
            .map(|s| s.key)
            .collect();
        assert!(keys.contains(&"ezpl.media_type".into()));
        assert!(!keys.contains(&"media.sense_mode".into()));
    }

    #[test]
    fn test_label_scales_with_dpi_and_drops_zpl_characters() {
        let lines = ["2x1 labels".to_string(), "a^b~c".to_string()];
        assert_eq!(
            test_label(&lines, 203),
            "^XA^CI28^FO20,20^A0N,32,32^FD2x1 labels^FS^FO20,58^A0N,24,24^FDabc^FS^XZ"
        );
        assert!(test_label(&lines, 300).contains("^FO29,29^A0N,47,47"));
    }
}
