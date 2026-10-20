use crate::{printer, setup, worker};
use serde::Deserialize;
use std::time::Duration;

// the buttons on the printer page; calibrating has its own command, since it needs a method
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Feed,
    ConfigLabel,
    NetworkLabel,
    SensorProfile,
    TestLabel,
    Pause,
    Resume,
    CancelJobs,
    ClearError,
    Restart,
}

impl Action {
    // what goes to the printer, for the actions that are a single command
    fn command(self) -> Option<&'static [u8]> {
        Some(match self {
            Action::Feed => b"~PH", // slew to the next label
            Action::ConfigLabel => b"~WC",
            Action::NetworkLabel => b"~WL",
            Action::SensorProfile => b"~JG", // prints the sensor readings across a few labels
            Action::Pause => b"! U1 do \"device.pause\" \"\"\r\n",
            Action::Resume => b"! U1 do \"device.unpause\" \"\"\r\n",
            Action::CancelJobs => b"~JA",
            Action::Restart => b"! U1 do \"device.reset\" \"\"\r\n",
            Action::TestLabel | Action::ClearError => return None,
        })
    }
}

// returns what to tell the user once it's done
pub async fn run(ip: &str, action: Action) -> Result<String, String> {
    let failed = |e: anyhow::Error| e.to_string();
    if let Some(bytes) = action.command() {
        printer::send(ip, bytes).await.map_err(failed)?;
        return Ok(match action {
            Action::Restart => "Restarting; it takes about a minute to come back".into(),
            _ => "Sent".into(),
        });
    }
    match action {
        Action::TestLabel => {
            let got = printer::query(
                ip,
                &[
                    "head.resolution.in_dpi",
                    "device.product_name",
                    "device.unique_id",
                ],
            )
            .await
            .map_err(failed)?;
            let get = |k: &str| got[k].as_str().unwrap_or("").trim().to_string();
            let dpi = get("head.resolution.in_dpi").parse().unwrap_or(203);
            let lines = [
                "Test label".to_string(),
                format!("{} {}", get("device.product_name"), get("device.unique_id")),
                format!("{dpi} dpi"),
                "zebra_config test print".into(),
            ];
            printer::send(ip, setup::test_label(&lines, dpi).as_bytes())
                .await
                .map_err(failed)?;
            Ok("Sent".into())
        }
        Action::ClearError => {
            worker::recover(ip, &|_| {})
                .await
                .map_err(|e| e.into_message())?;
            Ok("Cleared and paused".into())
        }
        _ => unreachable!("single commands are handled above"),
    }
}

// how to calibrate: auto-detect can mistake the gaps between wristbands for the gaps of
// gap/notch media, so the method can be chosen
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Calibration {
    Gap,
    Mark,
    Continuous,
    Auto,
}

impl Calibration {
    // the media settings to calibrate with; auto leaves the type to the printer
    fn settings(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Calibration::Gap => &[
                ("ezpl.media_type", "gap/notch"),
                ("media.sense_mode", "gap"),
                ("device.sensor_select", "transmissive"),
            ],
            Calibration::Mark => &[
                ("ezpl.media_type", "mark"),
                ("media.sense_mode", "bar"),
                ("device.sensor_select", "reflective"),
            ],
            Calibration::Continuous => &[("ezpl.media_type", "continuous")],
            Calibration::Auto => &[("ezpl.media_type", "auto_detect")],
        }
    }
}

// sets the media type for the method, makes sure it took, then calibrates
pub async fn calibrate(ip: &str, how: Calibration) -> Result<String, String> {
    let failed = |e: anyhow::Error| e.to_string();
    let lines: String = how
        .settings()
        .iter()
        .map(|(k, v)| format!("! U1 setvar \"{k}\" \"{v}\"\r\n"))
        .collect();
    printer::send(ip, lines.as_bytes()).await.map_err(failed)?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let keys: Vec<&str> = how.settings().iter().map(|(k, _)| *k).collect();
    let got = printer::query(ip, &keys).await.map_err(failed)?;
    let refused: Vec<&str> = how
        .settings()
        .iter()
        .filter(|(k, v)| !got[k].as_str().is_some_and(|x| worker::same_value(x, v)))
        .map(|(k, _)| *k)
        .collect();
    if !refused.is_empty() {
        return Err(format!("the printer didn't take {}", refused.join(", ")));
    }

    printer::send(ip, b"~JC").await.map_err(failed)?;
    let h = printer::wait_idle(ip, 60).await.map_err(failed)?;
    if !h.errors.is_empty() {
        return Err(format!("calibration stopped with {}", h.errors.join(", ")));
    }
    let got = printer::query(ip, &["zpl.label_length", "head.resolution.in_dpi"])
        .await
        .map_err(failed)?;
    let num = |k: &str| got[k].as_str().and_then(|v| v.trim().parse::<f64>().ok());
    Ok(
        match (num("zpl.label_length"), num("head.resolution.in_dpi")) {
            (Some(dots), Some(dpi)) if dpi > 0.0 => {
                format!(
                    "Calibrated: labels measure {dots:.0} dots ({:.2} in)",
                    dots / dpi
                )
            }
            _ => "Calibrated".into(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_the_right_commands() {
        let parse = |s: &str| serde_json::from_str::<Action>(&format!("\"{s}\"")).unwrap();
        assert_eq!(parse("config_label").command(), Some(&b"~WC"[..]));
        assert_eq!(parse("feed").command(), Some(&b"~PH"[..]));
        assert_eq!(parse("cancel_jobs").command(), Some(&b"~JA"[..]));
        assert!(parse("clear_error").command().is_none());
        assert!(serde_json::from_str::<Action>("\"factory_reset\"").is_err());
        assert!(serde_json::from_str::<Action>("\"calibrate\"").is_err());
    }

    #[test]
    fn calibration_methods_set_matching_media_settings() {
        let mark = Calibration::Mark.settings();
        assert!(mark.contains(&("ezpl.media_type", "mark")));
        assert!(mark.contains(&("device.sensor_select", "reflective")));
        assert_eq!(
            Calibration::Auto.settings(),
            &[("ezpl.media_type", "auto_detect")]
        );
        let parsed: Calibration = serde_json::from_str("\"gap\"").unwrap();
        assert_eq!(parsed, Calibration::Gap);
    }
}
