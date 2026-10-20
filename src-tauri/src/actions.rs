use crate::{printer, setup, worker};
use serde::Deserialize;

// the buttons on the printer page
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Calibrate,
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
            Action::Calibrate => b"~JC",
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
    }
}
