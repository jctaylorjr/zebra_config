use crate::{
    AppState, media,
    model::{Printer, Profile, Status},
    printer, settings, usb,
};
use std::{collections::HashMap, path::Path, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

enum ApplyError {
    Unreachable(String), // retry
    Mismatch(String),    // printer answered but didn't take the setting: don't retry
    Unverified(String), // printer stopped answering partway through, e.g. it joined another network: don't retry
}

// change a printer and send the whole row to the GUI
pub fn update(app: &AppHandle, ip: &str, f: impl FnOnce(&mut Printer)) {
    let state = app.state::<AppState>();
    if let Some(p) = state.printers.lock().unwrap().get_mut(ip) {
        f(p);
        let _ = app.emit("printer-update", p.clone());
    }
}

pub fn set_status(app: &AppHandle, ip: &str, status: Status) {
    update(app, ip, |p| p.status = status);
}

pub async fn apply_with_retry(
    app: AppHandle,
    ip: String,
    profile: Profile,
    folder: Option<settings::Folder>,
) {
    let report = |s: Status| set_status(&app, &ip, s);
    let mut attempt = 0u32;
    let mut delay = 5u64;
    loop {
        attempt += 1;
        report(Status::Applying("Connecting".into()));
        match apply_once(&ip, &profile, folder.as_ref(), &report).await {
            Ok(checked) => {
                if let Ok(name) = firmware_name(&ip).await {
                    update(&app, &ip, |p| p.firmware = Some(name));
                }
                return report(Status::Applied(checked));
            }
            Err(ApplyError::Mismatch(m)) => return report(Status::Failed(m)),
            Err(ApplyError::Unverified(m)) => return report(Status::Unverified(m)),
            Err(ApplyError::Unreachable(reason)) => {
                report(Status::Offline {
                    attempt,
                    retry_in_secs: delay,
                    reason,
                });
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(600); // back off, cap at 10 min
            }
        }
    }
}

// same order as a USB stick: firmware, files, commands; then the profile's own settings.
// returns what was checked
async fn apply_once(
    ip: &str,
    p: &Profile,
    folder: Option<&settings::Folder>,
    report: &(dyn Fn(Status) + Sync),
) -> Result<String, ApplyError> {
    let unreachable = |e: anyhow::Error| ApplyError::Unreachable(e.to_string());
    let step = |s: &str| report(Status::Applying(s.into()));

    let usb = match folder.and_then(|f| f.path.as_deref()) {
        Some(path) => usb::load(Path::new(path))
            .map_err(|e| ApplyError::Mismatch(format!("can't read {path}: {e}")))?,
        None => usb::Folder::default(),
    };

    // worked out first, so media that doesn't fit stops before anything is sent
    let mut extra = vec![];
    let mut label = None; // the test label, printed if there's media once everything is set
    if let Some(m) = &p.media {
        let got = printer::query(
            ip,
            &[
                "head.resolution.in_dpi",
                "device.product_name",
                "device.unique_id",
            ],
        )
        .await
        .map_err(unreachable)?;
        let get = |k: &str| got[k].as_str().unwrap_or("").trim().to_string();
        let dpi = get("head.resolution.in_dpi")
            .parse()
            .map_err(|_| ApplyError::Mismatch("printer didn't report its dpi".into()))?;
        let max = printer::print_width_max(ip).await.map_err(unreachable)?;
        let width = media::width(m, dpi, max).map_err(ApplyError::Mismatch)?;
        extra = media::commands(m, dpi, max).map_err(ApplyError::Mismatch)?;
        let lines = [
            p.name.clone(),
            format!("{} {}", get("device.product_name"), get("device.unique_id")),
            format!("{width} dots at {dpi} dpi"),
            "zebra_config test print".into(),
        ];
        label = Some(media::test_label(&lines, dpi));
    }
    extra.extend(p.commands.iter().cloned());

    let mut checked = vec![];
    if let Some(firmware) = &usb.firmware {
        let want = folder.and_then(|f| f.firmware_version.as_deref());
        let current = firmware_name(ip).await.map_err(unreachable)?;
        if want != Some(current.as_str()) {
            step("Installing firmware");
            printer::send(ip, firmware).await.map_err(unreachable)?;
            step("Waiting for restart");
            if !wait_for_restart(ip, want).await {
                return Err(ApplyError::Mismatch(match want {
                    Some(v) => format!("printer did not report firmware {v} within 20 min"),
                    None => "printer did not restart within 20 min of the firmware download".into(),
                }));
            }
            checked.push("firmware installed".to_string());
        }
    }

    if !usb.files.is_empty() {
        step("Sending files");
        for (name, data) in &usb.files {
            printer::send_file(ip, name, data)
                .await
                .map_err(unreachable)?;
        }
    }

    let mut commands: Vec<&[u8]> = usb.commands.iter().map(|(_, d)| d.as_slice()).collect();
    let extra = if extra.is_empty() {
        String::new()
    } else {
        extra.join("\r\n") + "\r\n"
    };
    if !extra.is_empty() {
        commands.push(extra.as_bytes());
    }
    if !commands.is_empty() {
        step("Sending commands");
    }
    for (i, data) in commands.iter().enumerate() {
        if let Err(e) = printer::send(ip, data).await {
            // an earlier command may have moved the printer to another network
            return Err(if i == 0 {
                unreachable(e)
            } else {
                ApplyError::Unverified(format!("printer stopped answering partway through: {e}"))
            });
        }
        if resets_printer(data) {
            step("Waiting for restart");
            if !wait_for_restart(ip, None).await {
                return Err(ApplyError::Unverified(
                    "printer did not come back after a command file reset it".into(),
                ));
            }
        }
    }

    step("Verifying");
    tokio::time::sleep(Duration::from_secs(1)).await; // let the printer apply
    let stopped = |e: anyhow::Error| {
        ApplyError::Unverified(format!(
            "sent everything, but the printer stopped answering: {e}"
        ))
    };

    let text: String = commands
        .iter()
        .map(|d| String::from_utf8_lossy(d))
        .collect();
    let expected = expected_settings(&text);
    if !expected.is_empty() {
        let keys: Vec<&str> = expected.iter().map(|(k, _)| k.as_str()).collect();
        let got = printer::query(ip, &keys).await.map_err(stopped)?;
        let bad: Vec<&str> = expected
            .iter()
            .filter(|(k, v)| {
                !got.get(k)
                    .and_then(|x| x.as_str())
                    .is_some_and(|x| x.trim().eq_ignore_ascii_case(v))
            })
            .map(|(k, _)| k.as_str())
            .collect();
        if !bad.is_empty() {
            return Err(ApplyError::Mismatch(format!(
                "did not read back: {}",
                bad.join(", ")
            )));
        }
        checked.push(format!("{} settings", expected.len()));
    }

    // files/ must match in size; files uploaded by command files just have to exist
    let names: Vec<&str> = usb.files.iter().map(|(n, _)| n.as_str()).collect();
    let want = usb::expected_files(&names, &usb.commands);
    if !want.is_empty() {
        let sizes: HashMap<String, u64> = usb
            .files
            .iter()
            .map(|(n, d)| (n.to_uppercase(), d.len() as u64))
            .collect();
        let on_printer = printer::files(ip).await.map_err(stopped)?;
        let missing: Vec<&str> = want
            .iter()
            .filter(|n| match on_printer.get(*n) {
                None => true,
                Some(size) => sizes.get(*n).is_some_and(|want| want != size),
            })
            .map(|n| n.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(ApplyError::Mismatch(format!(
                "not on E: afterwards: {}",
                missing.join(", ")
            )));
        }
        checked.push(format!("{} on E:", want.join(", ")));
    }

    if let Some(label) = label {
        checked.push(calibrate_and_print(ip, &label, &step).await?);
    }

    Ok(if checked.is_empty() {
        "nothing to check".into()
    } else {
        checked.join("; ")
    })
}

// only feeds when there's media and nothing is wrong, since feeding with no media leaves the
// printer in an error that otherwise takes opening the head to clear
async fn calibrate_and_print(
    ip: &str,
    label: &str,
    step: &(dyn Fn(&str) + Sync),
) -> Result<String, ApplyError> {
    let stopped =
        |e: anyhow::Error| ApplyError::Unverified(format!("printer stopped answering: {e}"));
    step("Checking for media");
    let mut h = printer::health(ip).await.map_err(stopped)?;
    if !h.head_closed {
        return Ok("head open, so no calibration or test print".into());
    }
    if let Some(e) = h.errors {
        recover(ip, step).await?;
        return Ok(format!(
            "cleared a printer error ({e}) and paused it; no calibration or test print"
        ));
    }
    if !h.media {
        return Ok("no media, so no calibration or test print".into());
    }
    if h.paused {
        // unpausing is also when the printer itself checks for media
        printer::send(ip, b"! U1 do \"device.unpause\" \"\"\r\n")
            .await
            .map_err(stopped)?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        h = printer::health(ip).await.map_err(stopped)?;
        if h.errors.is_some() || h.paused {
            recover(ip, step).await?;
            return Ok(
                "printer found no media when unpaused, so no calibration or test print".into(),
            );
        }
    }

    for (what, data) in [("Calibrating", "~JC"), ("Printing test label", label)] {
        step(what);
        printer::send(ip, data.as_bytes()).await.map_err(stopped)?;
        // device.status reads "busy" until it's done
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            h = printer::health(ip).await.map_err(stopped)?;
            if h.status != "busy" {
                break;
            }
        }
        if h.errors.is_some() || h.paused || h.status != "ready" {
            let problem = h.errors.map_or(h.status, |e| format!("an error ({e})"));
            recover(ip, step).await?;
            return Err(ApplyError::Mismatch(format!(
                "{what} stopped with {problem}; cleared and paused the printer"
            )));
        }
    }
    Ok("calibrated, test label printed".into())
}

// leave the printer as if its head had been opened and closed: nothing queued, no error, paused.
// a reset is the only thing that clears a paper out without opening the head
async fn recover(ip: &str, step: &(dyn Fn(&str) + Sync)) -> Result<(), ApplyError> {
    let stopped =
        |e: anyhow::Error| ApplyError::Unverified(format!("printer stopped answering: {e}"));
    step("Clearing printer error");
    printer::send(ip, b"~JA~JR").await.map_err(stopped)?;
    if !wait_for_restart(ip, None).await {
        return Err(ApplyError::Unverified(
            "printer did not come back after resetting it to clear an error".into(),
        ));
    }
    printer::send(ip, b"! U1 do \"device.pause\" \"\"\r\n")
        .await
        .map_err(stopped)
}

async fn firmware_name(ip: &str) -> anyhow::Result<String> {
    let got = printer::query(ip, &["appl.name"]).await?;
    Ok(got["appl.name"].as_str().unwrap_or("").to_string())
}

// with a known firmware version, wait until the printer reports it; otherwise wait for it
// to drop off the network and come back. false after 20 min
async fn wait_for_restart(ip: &str, want: Option<&str>) -> bool {
    let mut went_down = false;
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_secs(10)).await;
        match firmware_name(ip).await {
            Ok(name) if want == Some(name.as_str()) => return true,
            Ok(_) if want.is_none() && went_down => return true,
            Ok(_) => {}
            Err(_) => went_down = true,
        }
    }
    false
}

// a command file can end by resetting the printer, e.g. {}{"device.reset" : ""} or ~JR
fn resets_printer(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(data);
    text.contains("\"device.reset\"") || text.contains("~JR")
}

// every setvar to read back, keeping the last value for each key. printers never read back
// passwords or wifi keys
fn expected_settings(text: &str) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = vec![];
    for (k, v) in text.lines().filter_map(parse_setvar) {
        if ["password", "psk", "passphrase"]
            .iter()
            .any(|s| k.contains(s))
        {
            continue;
        }
        match expected.iter_mut().find(|(ek, _)| *ek == k) {
            Some(e) => e.1 = v,
            None => expected.push((k, v)),
        }
    }
    expected
}

// ! U1 setvar "key" "value"  ->  ("key", "value")
fn parse_setvar(line: &str) -> Option<(String, String)> {
    if !line.contains("setvar") {
        return None;
    }
    let parts: Vec<&str> = line.split('"').collect();
    (parts.len() >= 4).then(|| (parts[1].to_string(), parts[3].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_setvar_lines() {
        assert_eq!(
            parse_setvar(r#"! U1 setvar "rtc.timezone" "EST5EDT,M3.2.0/2:00:00,M11.1.0/2:00:00""#),
            Some((
                "rtc.timezone".into(),
                "EST5EDT,M3.2.0/2:00:00,M11.1.0/2:00:00".into()
            ))
        );
        assert_eq!(parse_setvar(r#"! U1 do "file.delete" "E:A.TXT""#), None);
        assert_eq!(parse_setvar(r#"! U1 setvar "broken"#), None);
    }

    #[test]
    fn expected_settings_keep_the_last_value_and_skip_secrets() {
        let text = "! U1 setvar \"ezpl.media_type\" \"auto_detect\"\r\n\
                    ^XA^WSexamplenet,I,L,,,^XZ\r\n\
                    ! U1 setvar \"wlan.8021x.peap.password\" \"secret\"\r\n\
                    ! U1 setvar \"wlan.wpa.psk\" \"secret\"\r\n\
                    {}{\"weblink.ip.conn1.retry_interval\":\"36\"}\r\n\
                    ! U1 setvar \"ezpl.media_type\" \"gap/notch\"\r\n";
        assert_eq!(
            expected_settings(text),
            vec![("ezpl.media_type".to_string(), "gap/notch".to_string())]
        );
    }

    #[test]
    fn spots_command_files_that_reset_the_printer() {
        assert!(resets_printer(
            b"{}{\"file.cert.weblink1.ca_file\":\"CA.cer\"}{}{\"device.reset\" : \"\"}"
        ));
        assert!(resets_printer(b"! U1 do \"device.reset\" \"\"\r\n"));
        assert!(resets_printer(b"~JR"));
        assert!(!resets_printer(
            b"! U1 setvar \"device.reset_button\" \"on\"\r\n"
        ));
        assert!(!resets_printer(
            b"! U1 setvar \"power.energy_star.enable\" \"off\"\r\n"
        ));
    }

    // talks to a real printer, so it only runs when asked. both env vars after the IP are optional:
    // ZEBRA_TEST_IP=192.168.1.50 ZEBRA_TEST_FOLDER=/path/to/Zebra ZEBRA_TEST_PROFILE='{...profile json...}' \
    //   cargo test -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn apply_to_printer() {
        let ip = std::env::var("ZEBRA_TEST_IP").expect("set ZEBRA_TEST_IP");
        let folder = std::env::var("ZEBRA_TEST_FOLDER")
            .ok()
            .map(|path| settings::Folder {
                models: vec![],
                path: Some(path),
                firmware_version: None,
            });
        let p: Profile = std::env::var("ZEBRA_TEST_PROFILE")
            .map(|j| serde_json::from_str(&j).expect("bad ZEBRA_TEST_PROFILE"))
            .unwrap_or(Profile {
                id: "test".into(),
                name: "test".into(),
                models: vec![],
                media: None,
                commands: vec![],
            });
        let report = |s: Status| println!("{}", serde_json::to_string(&s).unwrap());
        match apply_once(&ip, &p, folder.as_ref(), &report).await {
            Ok(checked) => println!("applied: {checked}"),
            Err(
                ApplyError::Unreachable(e) | ApplyError::Mismatch(e) | ApplyError::Unverified(e),
            ) => {
                panic!("{e}")
            }
        }
    }
}
