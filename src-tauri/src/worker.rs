use crate::{
    AppState,
    model::{Profile, Status},
    printer,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

enum ApplyError {
    Unreachable(String), // retry
    Mismatch(String),    // printer answered but didn't take the setting: don't retry
}

pub fn set_status(app: &AppHandle, ip: &str, status: Status) {
    let state = app.state::<AppState>();
    if let Some(p) = state.printers.lock().unwrap().get_mut(ip) {
        p.status = status.clone();
    }
    let _ = app.emit("printer-update", (ip, status));
}

pub async fn apply_with_retry(app: AppHandle, ip: String, profile: Profile) {
    let mut attempt = 0u32;
    let mut delay = 5u64;
    loop {
        attempt += 1;
        set_status(&app, &ip, Status::Applying);
        match apply_once(&ip, &profile).await {
            Ok(()) => return set_status(&app, &ip, Status::Applied),
            Err(ApplyError::Mismatch(m)) => return set_status(&app, &ip, Status::Failed(m)),
            Err(ApplyError::Unreachable(_)) => {
                set_status(
                    &app,
                    &ip,
                    Status::Offline {
                        attempt,
                        retry_in_secs: delay,
                    },
                );
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(600); // back off, cap at 10 min
            }
        }
    }
}

async fn apply_once(ip: &str, p: &Profile) -> Result<(), ApplyError> {
    let unreachable = |e: anyhow::Error| ApplyError::Unreachable(e.to_string());

    let mut data = p.commands.join("\n");
    data.push('\n');
    printer::send(ip, data.as_bytes())
        .await
        .map_err(unreachable)?;

    // read back every setvar line and compare
    let expected: Vec<(String, String)> =
        p.commands.iter().filter_map(|l| parse_setvar(l)).collect();
    if expected.is_empty() {
        return Ok(());
    }
    tokio::time::sleep(Duration::from_millis(500)).await; // let the printer apply
    let keys: Vec<&str> = expected.iter().map(|(k, _)| k.as_str()).collect();
    let got = printer::query(ip, &keys).await.map_err(unreachable)?;

    let bad: Vec<&str> = expected
        .iter()
        .filter(|(k, v)| got.get(k).and_then(|x| x.as_str()) != Some(v.as_str()))
        .map(|(k, _)| k.as_str())
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(ApplyError::Mismatch(format!(
            "did not read back: {}",
            bad.join(", ")
        )))
    }
}

// ! U1 setvar "key" "value"  ->  ("key", "value")
fn parse_setvar(line: &str) -> Option<(String, String)> {
    if !line.contains("setvar") {
        return None;
    }
    let parts: Vec<&str> = line.split('"').collect();
    (parts.len() >= 4).then(|| (parts[1].to_string(), parts[3].to_string()))
}
