use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
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

// group 1 of the ~HQES bits, from the ZPL manual's ~HQ page. bits whose group the manual leaves
// unclear are reported as "other" with their value rather than guessed at
const ERRORS: [(u64, &str); 10] = [
    (0x1, "media out"),
    (0x2, "ribbon out"),
    (0x4, "head open"),
    (0x8, "cutter fault"),
    (0x10, "printhead over temperature"),
    (0x20, "motor over temperature"),
    (0x40, "bad printhead element"),
    (0x80, "printhead detection error"),
    (0x100, "invalid firmware configuration"),
    (0x200, "printhead thermistor open"),
];
const WARNINGS: [(u64, &str); 3] = [
    (0x1, "needs media calibration"),
    (0x2, "clean printhead"),
    (0x4, "replace printhead"),
];

pub struct Alerts {
    pub errors: Vec<String>, // besides pause
    pub warnings: Vec<String>,
    pub paused: bool,
}

// a ~HQES reply is complete once its warnings line has the flag and both groups
pub fn hqes_complete(text: &str) -> bool {
    text.split("WARNINGS:")
        .nth(1)
        .is_some_and(|w| w.split_whitespace().count() >= 3)
}

pub fn alerts(hqes: &str) -> Option<Alerts> {
    let (errors, warnings) = parse_hqes(hqes)?;
    Some(Alerts {
        errors: names(errors & !PAUSED, &ERRORS, "error"),
        warnings: names(warnings, &WARNINGS, "warning"),
        paused: errors & PAUSED != 0,
    })
}

pub async fn health(ip: &str) -> anyhow::Result<Health> {
    let got = query(
        ip,
        &[
            "device.status",
            "sensor.paper_supply",
            "head.latch",
            "sensor.gap.cur",
            "sensor.gap.ppr_out_thold",
            "zpl.system_status",
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

    // the SGD version of ~HQES answers whatever command language the printer is set to, which
    // matters for mobile printers in line print mode; ~HQES is for printers without it
    let a = match got["zpl.system_status"].as_str().and_then(system_status) {
        Some(a) => a,
        None => {
            let text = ask(ip, b"~HQES", hqes_complete).await?;
            alerts(&text).ok_or_else(|| anyhow::anyhow!("no error status from {ip}"))?
        }
    };

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
        paused: a.paused,
        errors: a.errors,
        warnings: a.warnings,
    })
}

// after a feed or print starts, device.status reads "busy" until it's done. checks once a
// second, for up to `max_secs`. a printer that's calibrating can be too busy to answer, so only
// the last check counts
pub async fn wait_idle(ip: &str, max_secs: u32) -> anyhow::Result<Health> {
    let start = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let h = health(ip).await;
        let idle = matches!(&h, Ok(h) if h.status != "busy");
        if idle || start.elapsed() >= Duration::from_secs(max_secs.max(1).into()) {
            return h;
        }
    }
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

// "1,1,00000000,00010000,0,00000000,00000000": the printer status flag, then the error flag
// and its two groups, then the warning flag and its two groups; the same bits as ~HQES
pub fn system_status(text: &str) -> Option<Alerts> {
    let f: Vec<&str> = text.trim().split(',').map(str::trim).collect();
    let [_, _, eh, el, _, wh, wl] = f[..] else {
        return None;
    };
    let hex = |g: &str| u64::from_str_radix(g, 16).ok();
    let errors = (hex(eh)? << 32) | hex(el)?;
    let warnings = (hex(wh)? << 32) | hex(wl)?;
    Some(Alerts {
        errors: names(errors & !PAUSED, &ERRORS, "error"),
        warnings: names(warnings, &WARNINGS, "warning"),
        paused: errors & PAUSED != 0,
    })
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
    let text = read(
        ip,
        b"! U1 getvar \"allcv\"\r\n",
        Duration::from_secs(5),
        |t| t.ends_with("\r\n\"\""),
    )
    .await?;
    Ok(parse_allcv(&text))
}

// "ezpl.print_width : 406 , Choices: 2-448", "device.friendly_name : CSCABC123456789" and
// "device.location :  ";
// branch names ("ezpl.") and commands ("file.delete") have no value and are left out
fn parse_allcv(text: &str) -> Vec<Setting> {
    // a name listed twice keeps its first value, so each name is one row on the Printer page
    let mut seen = std::collections::HashSet::new();
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
        .filter(|s| seen.insert(s.key.clone()))
        .collect()
}

// send anything and return whatever the printer says back, for the printer page's command box
pub async fn raw(ip: &str, data: &[u8]) -> anyhow::Result<String> {
    read(ip, data, Duration::from_millis(1500), |_| false).await
}

// send on 9100 and read until `done` says the reply is complete or the printer has been quiet
// for `idle`, for replies that may have no end marker. gives up reading after 20 s
pub async fn read(
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

// the ~HS flags worth knowing about; see the ZPL manual's ~HS page for every field
#[derive(Debug, Default, PartialEq)]
pub struct HostStatus {
    pub paper_out: bool,
    pub paused: bool,
    pub label_length: u32, // dots
    pub formats: u32,      // waiting in the receive buffer
    pub buffer_full: bool,
    pub comm_diagnostics: bool,
    pub partial_format: bool,
    pub corrupt_ram: bool,
    pub under_temperature: bool,
    pub over_temperature: bool,
    pub head_up: bool,
    pub ribbon_out: bool,
    pub label_waiting: bool,
    pub labels_remaining: u32,
}

// a ~HS reply is three lines, each wrapped in STX and ETX
pub fn hs_complete(text: &str) -> bool {
    text.matches('\u{3}').count() >= 3 || text.lines().filter(|l| l.contains(',')).count() >= 3
}

// "aaa,b,c,dddd,eee,f,g,h,iii,j,k,l" then "mmm,n,o,p,q,r,s,t,uuuuuuuu,v,www" then "xxxx,y"
pub fn parse_hs(text: &str) -> Option<HostStatus> {
    let lines: Vec<Vec<String>> = text
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_control() || c.is_whitespace()))
        .filter(|l| l.contains(','))
        .map(|l| l.split(',').map(|f| f.trim().to_string()).collect())
        .collect();
    let (one, two) = (lines.first()?, lines.get(1)?);
    if one.len() < 12 || two.len() < 9 {
        return None;
    }
    let flag = |f: &String| f == "1";
    let num = |f: &String| f.parse().unwrap_or(0);
    Some(HostStatus {
        paper_out: flag(&one[1]),
        paused: flag(&one[2]),
        label_length: num(&one[3]),
        formats: num(&one[4]),
        buffer_full: flag(&one[5]),
        comm_diagnostics: flag(&one[6]),
        partial_format: flag(&one[7]),
        corrupt_ram: flag(&one[9]),
        under_temperature: flag(&one[10]),
        over_temperature: flag(&one[11]),
        head_up: flag(&two[2]),
        ribbon_out: flag(&two[3]),
        label_waiting: flag(&two[7]),
        labels_remaining: num(&two[8]),
    })
}

// "Head Temp = 30" style lines, as ~HD prints them
pub fn parse_pairs(text: &str, separator: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(separator)?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

// the printhead element test summary from ~HQJT, e.g. "0,A,0021,0447,0000"
#[derive(Debug, PartialEq)]
pub struct HeadTest {
    pub failures: u32,
    pub first: u32,
    pub last: u32,
}

pub fn hqjt_complete(text: &str) -> bool {
    text.lines().any(|l| l.trim().matches(',').count() == 4)
}

pub fn parse_hqjt(text: &str) -> Option<HeadTest> {
    let line = text.lines().find(|l| l.trim().matches(',').count() == 4)?;
    let f: Vec<&str> = line.trim().split(',').collect();
    Some(HeadTest {
        failures: f[0].trim().parse().ok()?,
        first: f[2].trim().parse().ok()?,
        last: f[3].trim().parse().ok()?,
    })
}

// a distance as ~HQPH, ~HQOD and ~HQMA print it, e.g. "43804 \"" (inches) or "50 km", in metres
pub fn parse_distance(text: &str) -> Option<f64> {
    let text = text.trim();
    let digits: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let n: f64 = digits.parse().ok()?;
    let unit = text[digits.len()..].trim().to_ascii_lowercase();
    Some(match unit.as_str() {
        "\"" | "in" | "inches" => n * 0.0254,
        "cm" => n / 100.0,
        "mm" => n / 1000.0,
        "km" => n * 1000.0,
        "m" | "meters" | "metres" => n,
        _ => return None,
    })
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
            ["invalid firmware configuration"]
        );
        assert_eq!(
            names(0x400, &ERRORS, "error"),
            ["other error 0000000000000400"]
        );
        assert_eq!(
            names(0x8, &WARNINGS, "warning"),
            ["other warning 0000000000000008"]
        );
        assert_eq!(parse_hqes("no status here"), None);
    }

    #[test]
    fn reads_the_sgd_system_status_like_hqes() {
        // captured from a ZD410 while paused
        let paused = system_status("1,1,00000000,00010000,0,00000000,00000000").unwrap();
        assert!(paused.paused && paused.errors.is_empty() && paused.warnings.is_empty());
        let out = system_status("1,1,00000000,00000005,1,00000000,00000002").unwrap();
        assert_eq!(out.errors, ["media out", "head open"]);
        assert_eq!(out.warnings, ["clean printhead"]);
        assert!(system_status("?").is_none());
        assert!(system_status("0,0,00000000").is_none());
    }

    #[test]
    fn reads_the_full_settings_list() {
        let allcv = "\"ezpl.\r\nezpl.print_width : 406 , Choices: 2-448\r\ndevice.friendly_name : CSCABC123456789\r\nfile.delete\r\nip.mirror.password : * \r\nprint.tone_format : zpl , Choices: zpl\r\ndevice.location :  \r\ndevice.print_out_report, Choices: settings,network\r\n";
        let s = parse_allcv(allcv);
        assert_eq!(s.len(), 5);
        assert_eq!(
            (s[4].key.as_str(), s[4].value.as_str()),
            ("device.location", "")
        );
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
            ("CSCABC123456789", None)
        );
        assert_eq!(s[2].value, "*");
        let twice = parse_allcv("a.b : 1\r\na.b : 2\r\n");
        assert_eq!((twice.len(), twice[0].value.as_str()), (1, "1"));
    }

    #[test]
    fn reads_host_status() {
        let hs = "\u{2}030,0,1,0212,002,0,0,0,000,0,0,0\u{3}\r\n\u{2}000,0,0,0,0,2,3,0,00000000,1,000\u{3}\r\n\u{2}1234,0\u{3}\r\n";
        assert!(hs_complete(hs));
        let s = parse_hs(hs).unwrap();
        assert!(s.paused && !s.paper_out && !s.corrupt_ram && !s.head_up);
        assert_eq!((s.label_length, s.formats, s.labels_remaining), (212, 2, 0));
        assert!(!hs_complete("\u{2}030,0,1,0212"));
        assert_eq!(parse_hs("nothing"), None);
    }

    #[test]
    fn reads_head_diagnostics_and_the_element_test() {
        let hd = "\r\nHead Temp = 30\r\nAmbient Temp = 00\r\nHead Test = Passed\r\nDarkness Adjust = 25\r\n";
        let pairs = parse_pairs(hd, " = ");
        assert_eq!(pairs["Head Test"], "Passed");
        assert_eq!(pairs["Head Temp"], "30");
        let jt = "\r\n\r\n  HEAD TEST RESULTS                         \r\n   0,A,0021,0447,0000                       \r\n";
        assert!(hqjt_complete(jt));
        assert_eq!(
            parse_hqjt(jt),
            Some(HeadTest {
                failures: 0,
                first: 21,
                last: 447
            })
        );
    }

    #[test]
    fn reads_printed_distances() {
        let close = |a: Option<f64>, b: f64| (a.unwrap() - b).abs() < 0.01;
        assert!(close(parse_distance("43804 \"      "), 1112.62));
        assert!(close(parse_distance("50 km"), 50000.0));
        assert!(close(parse_distance("120 cm"), 1.2));
        assert_eq!(parse_distance("lots"), None);
    }
}
