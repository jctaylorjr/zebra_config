use std::{collections::HashMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

async fn connect(ip: &str, port: u16) -> anyhow::Result<TcpStream> {
    Ok(timeout(Duration::from_secs(5), TcpStream::connect((ip, port))).await??)
}

pub async fn query(ip: &str, keys: &[&str]) -> anyhow::Result<serde_json::Value> {
    let mut s = connect(ip, 9200).await?;
    let req: serde_json::Map<String, serde_json::Value> = keys
        .iter()
        .map(|k| (k.to_string(), serde_json::Value::Null))
        .collect();
    s.write_all(format!("{{}}{}", serde_json::Value::Object(req)).as_bytes())
        .await?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(3), s.read(&mut chunk)).await??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.iter().position(|&b| b == b'{') {
            if let Ok(v) = serde_json::from_slice(&buf[i..]) {
                return Ok(v);
            }
        }
    }
    anyhow::bail!("no valid response from {ip}")
}

pub async fn send(ip: &str, data: &[u8]) -> anyhow::Result<()> {
    let mut s = connect(ip, 9100).await?;
    // generous, since a firmware file is tens of MB and the printer takes it slowly
    timeout(Duration::from_secs(900), s.write_all(data)).await??;
    s.shutdown().await?;
    Ok(())
}

// send a command on 9100 and read the reply until `done` says it's complete
async fn ask(ip: &str, command: &[u8], done: impl Fn(&str) -> bool) -> anyhow::Result<String> {
    let mut s = connect(ip, 9100).await?;
    s.write_all(command).await?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while !done(&String::from_utf8_lossy(&buf)) {
        let n = timeout(Duration::from_secs(3), s.read(&mut chunk)).await??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

// store a file on E:, like the printer does with a USB mirror's files/ folder
pub async fn send_file(ip: &str, name: &str, data: &[u8]) -> anyhow::Result<()> {
    let mut msg = format!(
        "! CISDFCRC16\r\n0000\r\n{name}\r\n{:08X}\r\n0000\r\n",
        data.len()
    )
    .into_bytes();
    msg.extend_from_slice(data);
    send(ip, &msg).await
}

// name -> size of everything on E:
pub async fn files(ip: &str) -> anyhow::Result<HashMap<String, u64>> {
    // the listing ends with the free-space line
    let text = ask(ip, b"! U1 do \"file.dir\" \"E:\"\r\n", |t| {
        t.contains("bytes free")
    })
    .await?;
    Ok(parse_dir(&text))
}

// "* E:FEEDBACK.GET   389" lines
fn parse_dir(text: &str) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|l| {
            let mut words = l.trim().strip_prefix("* E:")?.split_whitespace();
            Some((words.next()?.to_string(), words.next()?.parse().ok()?))
        })
        .collect()
}

// the largest ezpl.print_width the printer accepts, from its allcv listing
pub async fn print_width_max(ip: &str) -> anyhow::Result<u32> {
    let text = ask(ip, b"! U1 getvar \"allcv\"\r\n", |t| {
        t.find("ezpl.print_width ")
            .is_some_and(|i| t[i..].contains('\n'))
    })
    .await?;
    parse_print_width_max(&text)
        .ok_or_else(|| anyhow::anyhow!("{ip} didn't report its print width range"))
}

// "ezpl.print_width : 406 , Choices: 2-448" -> 448
fn parse_print_width_max(allcv: &str) -> Option<u32> {
    let line = allcv
        .lines()
        .find(|l| l.trim_start().starts_with("ezpl.print_width "))?;
    line.split("Choices:")
        .nth(1)?
        .split('-')
        .nth(1)?
        .trim()
        .parse()
        .ok()
}

// what decides whether it's safe to feed media
pub struct Health {
    pub status: String, // device.status, e.g. "ready" or "busy"
    pub media: bool,
    pub head_closed: bool,
    pub paused: bool,
    pub errors: Vec<String>,   // besides pause, e.g. "media out"
    pub warnings: Vec<String>, // e.g. "clean printhead"
}

const PAUSED: u64 = 0x10000; // ~HQES lists pause as an error

// ~HQES bits, from the ZPL manual
const ERRORS: [(u64, &str); 8] = [
    (0x1, "media out"),
    (0x2, "ribbon out"),
    (0x4, "head open"),
    (0x8, "cutter fault"),
    (0x10, "printhead over temperature"),
    (0x20, "motor over temperature"),
    (0x40, "bad printhead element"),
    (0x80, "printhead detection error"),
];
const WARNINGS: [(u64, &str); 4] = [
    (0x1, "needs media calibration"),
    (0x2, "clean printhead"),
    (0x4, "replace printhead"),
    (0x8, "paper near end"),
];

pub async fn health(ip: &str) -> anyhow::Result<Health> {
    let got = query(
        ip,
        &[
            "device.status",
            "sensor.paper_supply",
            "head.latch",
            "sensor.gap.cur",
            "sensor.gap.ppr_out_thold",
        ],
    )
    .await?;
    let get = |k: &str| {
        got.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let num = |k: &str| get(k).parse::<u32>().ok();

    // complete once the warnings line has its flag and both groups
    let text = ask(ip, b"~HQES", |t| {
        t.split("WARNINGS:")
            .nth(1)
            .is_some_and(|w| w.split_whitespace().count() >= 3)
    })
    .await?;
    let (errors, warnings) =
        parse_hqes(&text).ok_or_else(|| anyhow::anyhow!("no error status from {ip}"))?;

    // paper_supply only notices missing media when the printer tries to feed, but the
    // transmissive sensor sees straight through to the light whichever sensor is selected
    let see_through = matches!(
        (num("sensor.gap.cur"), num("sensor.gap.ppr_out_thold")),
        (Some(cur), Some(thold)) if cur >= thold
    );
    Ok(Health {
        status: get("device.status"),
        media: !see_through && get("sensor.paper_supply") != "out",
        head_closed: get("head.latch") == "ok",
        paused: errors & PAUSED != 0,
        errors: names(errors & !PAUSED, &ERRORS, "error"),
        warnings: names(warnings, &WARNINGS, "warning"),
    })
}

// "ERRORS:   1 00000000 00010001" then "WARNINGS:   0 00000000 00000000": a set flag, then two
// 32 bit groups of bits for each
fn parse_hqes(text: &str) -> Option<(u64, u64)> {
    let bits = |section: &str| -> Option<u64> {
        let groups: Vec<u64> = section
            .split_whitespace()
            .skip(1)
            .take(2)
            .map(|g| u64::from_str_radix(g, 16).ok())
            .collect::<Option<_>>()?;
        let [high, low] = groups[..] else {
            return None;
        };
        Some((high << 32) | low)
    };
    let after_errors = text.split("ERRORS:").nth(1)?;
    let mut parts = after_errors.split("WARNINGS:");
    let errors = bits(parts.next()?)?;
    let warnings = parts.next().and_then(bits).unwrap_or(0);
    Some((errors, warnings))
}

fn names(bits: u64, known: &[(u64, &str)], kind: &str) -> Vec<String> {
    let mut out: Vec<String> = known
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| name.to_string())
        .collect();
    let rest = known.iter().fold(bits, |b, (bit, _)| b & !bit);
    if rest != 0 {
        out.push(format!("other {kind} {rest:016X}"));
    }
    out
}

// one line of the printer's full settings list
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    pub value: String,
    pub choices: Option<String>,
}

// every setting with a value, as the printer lists them in allcv
pub async fn all_settings(ip: &str) -> anyhow::Result<Vec<Setting>> {
    // the listing ends with "" but pauses for a couple of seconds partway through
    let text = read_all(
        ip,
        b"! U1 getvar \"allcv\"\r\n",
        Duration::from_secs(5),
        |t| t.ends_with("\r\n\"\""),
    )
    .await?;
    Ok(parse_allcv(&text))
}

// "ezpl.print_width : 406 , Choices: 2-448", "device.friendly_name : CSC50J173900310" and
// "device.location :  ";
// branch names ("ezpl.") and commands ("file.delete") have no value and are left out
fn parse_allcv(text: &str) -> Vec<Setting> {
    text.lines()
        .filter_map(|line| {
            let (key, rest) = line.trim_start().split_once(" :")?;
            let (value, choices) = match rest.rsplit_once(", Choices:") {
                Some((v, c)) => (v, Some(c.trim().to_string())),
                None => (rest, None),
            };
            Some(Setting {
                // the whole listing is quoted, so the first key starts with "
                key: key.trim().trim_start_matches('"').to_string(),
                value: value.trim().to_string(),
                choices,
            })
        })
        .collect()
}

// send anything and return whatever the printer says back, for the printer page's command box
pub async fn raw(ip: &str, data: &[u8]) -> anyhow::Result<String> {
    read_all(ip, data, Duration::from_millis(1500), |_| false).await
}

// send on 9100 and read until `done` says the reply is complete or the printer has been quiet
// for `idle`, for replies that may have no end marker. gives up reading after 20 s
async fn read_all(
    ip: &str,
    command: &[u8],
    idle: Duration,
    done: impl Fn(&str) -> bool,
) -> anyhow::Result<String> {
    let mut s = connect(ip, 9100).await?;
    s.write_all(command).await?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        match timeout(idle, s.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Ok(Err(e)) => return Err(e.into()),
        }
        if done(&String::from_utf8_lossy(&buf)) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // replies captured from a ZD410 on V84.20.05Z

    #[test]
    fn reads_the_e_drive_listing() {
        let text = "\"\r\n- DIR E:*.* \r\n* E:CACERTSV.NRD      4668  P       \r\n* E:ZCTEST.TXT        24          \r\n\r\n-  66932224 bytes free E: ONBOARD FLASH \r\n\"";
        let files = parse_dir(text);
        assert_eq!(files.len(), 2);
        assert_eq!(files["CACERTSV.NRD"], 4668);
        assert_eq!(files["ZCTEST.TXT"], 24);
    }

    #[test]
    fn reads_the_print_width_range() {
        let allcv = "ezpl.power_up_action : no motion , Choices: calibrate,feed\r\nezpl.print_width : 406 , Choices: 2-448\r\nezpl.media_type : auto_detect , Choices: auto_detect,continuous,gap/notch,mark\r\n";
        assert_eq!(parse_print_width_max(allcv), Some(448));
        assert_eq!(parse_print_width_max("ezpl.print_width : 406\r\n"), None);
    }

    #[test]
    fn reads_error_and_warning_flags() {
        let status = |errors: &str, warnings: &str| {
            format!(
                "\r\n\r\n  PRINTER STATUS\r\n   ERRORS:         {errors}\r\n   WARNINGS:       {warnings}\r\n"
            )
        };
        assert_eq!(
            parse_hqes(&status("0 00000000 00000000", "0 00000000 00000000")),
            Some((0, 0))
        );
        let (errors, warnings) =
            parse_hqes(&status("1 00000000 00010001", "1 00000000 00000002")).unwrap();
        assert_eq!(errors & PAUSED, PAUSED);
        assert_eq!(names(errors & !PAUSED, &ERRORS, "error"), ["media out"]);
        assert_eq!(names(warnings, &WARNINGS, "warning"), ["clean printhead"]);
        assert_eq!(names(0x5, &ERRORS, "error"), ["media out", "head open"]);
        assert_eq!(
            names(0x100, &ERRORS, "error"),
            ["other error 0000000000000100"]
        );
        assert_eq!(parse_hqes("no status here"), None);
    }

    #[test]
    fn reads_the_full_settings_list() {
        let allcv = "\"ezpl.\r\nezpl.print_width : 406 , Choices: 2-448\r\ndevice.friendly_name : CSC50J173900310\r\nfile.delete\r\nip.mirror.password : * \r\nprint.tone_format : zpl , Choices: zpl\r\ndevice.location :  \r\ndevice.print_out_report, Choices: settings,network\r\n";
        let s = parse_allcv(allcv);
        assert_eq!(s.len(), 5);
        assert_eq!((s[4].key.as_str(), s[4].value.as_str()), ("device.location", ""));
        assert_eq!(
            s[0],
            Setting {
                key: "ezpl.print_width".into(),
                value: "406".into(),
                choices: Some("2-448".into())
            }
        );
        assert_eq!(
            (s[1].value.as_str(), s[1].choices.as_ref()),
            ("CSC50J173900310", None)
        );
        assert_eq!(s[2].value, "*");
    }
}
