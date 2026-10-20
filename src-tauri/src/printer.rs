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
    pub errors: Option<String>, // ~HQES error flags other than pause, when any are set
}

const PAUSED: u64 = 0x10000; // ~HQES lists pause as an error
const MEDIA_OUT: u64 = 0x1;

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

    let text = ask(ip, b"~HQES", |t| t.contains("WARNINGS")).await?;
    let bits = parse_hqes(&text).ok_or_else(|| anyhow::anyhow!("no error status from {ip}"))?;

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
        paused: bits & PAUSED != 0,
        errors: describe(bits & !PAUSED),
    })
}

// "ERRORS:         1 00000000 00010001": set flag, then two 32 bit groups of error bits
fn parse_hqes(text: &str) -> Option<u64> {
    let groups: Vec<u64> = text
        .split("ERRORS:")
        .nth(1)?
        .split("WARNINGS")
        .next()?
        .split_whitespace()
        .skip(1)
        .map(|g| u64::from_str_radix(g, 16).ok())
        .collect::<Option<_>>()?;
    let [high, low] = groups[..] else {
        return None;
    };
    Some((high << 32) | low)
}

fn describe(bits: u64) -> Option<String> {
    match bits {
        0 => None,
        MEDIA_OUT => Some("media out".into()),
        _ => Some(format!("{:08X} {:08X}", bits >> 32, bits & 0xFFFF_FFFF)),
    }
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
    fn reads_error_flags_and_names_paper_out() {
        let status = |errors: &str| {
            format!(
                "\r\n\r\n  PRINTER STATUS\r\n   ERRORS:         {errors}\r\n   WARNINGS:       0 00000000 00000000\r\n"
            )
        };
        assert_eq!(parse_hqes(&status("0 00000000 00000000")), Some(0));
        let stuck = parse_hqes(&status("1 00000000 00010001")).unwrap();
        assert_eq!(stuck & PAUSED, PAUSED);
        assert_eq!(describe(stuck & !PAUSED).as_deref(), Some("media out"));
        assert_eq!(describe(0x4).as_deref(), Some("00000000 00000004"));
        assert_eq!(parse_hqes("no status here"), None);
    }
}
