use crate::{
    model::{self, Profile},
    printer, settings, setup, worker,
};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Connection, // always runs first, so it isn't in the list to pick from
    Alerts,
    HostStatus,
    Printhead,
    Media,
    Sensors,
    Profile,
    Memory,
    Network,
    Weblink,
    Clock,
    Wear,
    Restarts,
    FeedTest,
    CalibrationTest,
    ElementTest,
    SensorProfile,
}

#[derive(Serialize)]
pub struct CheckInfo {
    pub id: Check,
    pub label: &'static str,
    pub about: &'static str,
    pub feeds: bool, // feeds or prints media, so it's off unless picked
}

// in the order they run
pub fn catalogue() -> Vec<CheckInfo> {
    use Check::*;
    let c = |id, label, about, feeds| CheckInfo {
        id,
        label,
        about,
        feeds,
    };
    vec![
        c(
            Alerts,
            "Errors and warnings",
            "What the printer reports right now (~HQES)",
            false,
        ),
        c(
            HostStatus,
            "Printer state",
            "Paused, jobs stuck in the buffer, lost settings, temperature and diagnostics mode (~HS)",
            false,
        ),
        c(
            Printhead,
            "Printhead",
            "Head temperature and the last printhead element test (~HD, ~HQJT)",
            false,
        ),
        c(
            Media,
            "Media settings",
            "Media type, sensor and print width agree with each other",
            false,
        ),
        c(
            Sensors,
            "Media sensor",
            "Whether the sensor sees media, and its calibration levels (^HH)",
            false,
        ),
        c(
            Profile,
            "Profile settings",
            "The printer still has the chosen profile's settings",
            false,
        ),
        c(Memory, "Memory", "Free RAM and flash", false),
        c(
            Network,
            "Network",
            "Connection type, Wi-Fi signal, network errors and DHCP host name",
            false,
        ),
        c(
            Weblink,
            "Weblink",
            "Whether a configured weblink server is connected",
            false,
        ),
        c(
            Clock,
            "Clock",
            "The printer's clock against this computer's; weblink's secure connection needs it right",
            false,
        ),
        c(
            Wear,
            "Printhead wear",
            "Distance printed since cleaning and on this printhead (~HQPH, ~HQMA)",
            false,
        ),
        c(
            Restarts,
            "Restarts",
            "Why the printer last restarted (log.reboot.report)",
            false,
        ),
        c(
            FeedTest,
            "Feed a label",
            "Feeds one label and checks for errors: tests the motor, platen roller and media path",
            true,
        ),
        c(
            CalibrationTest,
            "Calibrate",
            "Calibrates with the current media settings and checks the measured label length",
            true,
        ),
        c(
            ElementTest,
            "Printhead element test",
            "Prints Zebra's element-out label while the printer tests every printhead dot",
            true,
        ),
        c(
            SensorProfile,
            "Sensor profile",
            "Prints a graph of what the media sensor sees, for media that won't calibrate",
            true,
        ),
    ]
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Ok,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Link {
    pub title: &'static str,
    pub url: &'static str,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fix {
    Resume,
    ClearError,
    CancelJobs,
    Restart,
    Calibrate,
    MatchSensor { mark: bool },
    PrintWidth { dots: u32 },
    SetClock { offset_minutes: i32 }, // the printer's time zone, minutes east of UTC
    StopCommDiagnostics,
    ApplySettings { settings: Vec<(String, String)> },
}

impl Fix {
    fn label(&self) -> String {
        match self {
            Fix::Resume => "Resume printing".into(),
            Fix::ClearError => "Clear the error".into(),
            Fix::CancelJobs => "Cancel the waiting jobs".into(),
            Fix::Restart => "Restart the printer".into(),
            Fix::Calibrate => "Calibrate".into(),
            Fix::MatchSensor { mark: true } => "Use the reflective sensor".into(),
            Fix::MatchSensor { mark: false } => "Use the transmissive sensor".into(),
            Fix::PrintWidth { dots } => format!("Set print width to {dots} dots"),
            Fix::SetClock { .. } => "Set the clock from this computer".into(),
            Fix::StopCommDiagnostics => "Turn off diagnostics mode".into(),
            Fix::ApplySettings { settings } => format!("Set the {} settings again", settings.len()),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Finding {
    pub check: Check,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub fix: Option<Fix>,
    pub fix_label: Option<String>,
    pub links: Vec<Link>,
}

impl Finding {
    fn fix(&mut self, fix: Fix) -> &mut Self {
        self.fix_label = Some(fix.label());
        self.fix = Some(fix);
        self
    }

    fn links(&mut self, links: Vec<Link>) -> &mut Self {
        self.links = links;
        self
    }
}

// one line of the run's log
#[derive(Serialize, Clone, Debug)]
pub struct LogLine {
    pub kind: &'static str, // step, send, reply, ok, info, warn, error
    pub text: String,
}

pub struct Context {
    pub profile: Option<Profile>,
    pub folders: Vec<settings::Folder>,
    pub pc_offset_minutes: i32, // this computer's time zone, minutes east of UTC
}

// what the connection check learns, for the checks after it
struct Info {
    model: String,
    serial: String,
    dpi: u32,
    ip_hostname: bool,
}

struct Session<'a> {
    ip: &'a str,
    log: &'a (dyn Fn(LogLine) + Sync),
    findings: Vec<Finding>,
    zt: bool, // a ZT printer, for which guide to link
}

impl Session<'_> {
    fn line(&self, kind: &'static str, text: impl Into<String>) {
        (self.log)(LogLine {
            kind,
            text: text.into(),
        });
    }

    fn find(
        &mut self,
        check: Check,
        severity: Severity,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> &mut Finding {
        let title = title.into();
        let kind = match severity {
            Severity::Error => "error",
            Severity::Warning => "warn",
            Severity::Info => "info",
            Severity::Ok => "ok",
        };
        self.line(kind, title.clone());
        self.findings.push(Finding {
            check,
            severity,
            title,
            detail: detail.into(),
            fix: None,
            fix_label: None,
            links: vec![],
        });
        self.findings.last_mut().unwrap()
    }

    async fn get(&self, keys: &[&str]) -> anyhow::Result<serde_json::Value> {
        self.line("send", format!("getvar {}", keys.join(" ")));
        let got = printer::query(self.ip, keys).await;
        match &got {
            Ok(v) => {
                for k in keys {
                    self.line("reply", format!("{k} = {}", shown(&v[k])));
                }
            }
            Err(e) => self.line("error", e.to_string()),
        }
        got
    }

    async fn ask(
        &self,
        command: &str,
        idle_ms: u64,
        done: impl Fn(&str) -> bool,
    ) -> anyhow::Result<String> {
        self.line("send", command.trim());
        let reply = printer::read(
            self.ip,
            command.as_bytes(),
            Duration::from_millis(idle_ms),
            done,
        )
        .await;
        match &reply {
            Ok(text) => {
                let lines: Vec<&str> = text
                    .lines()
                    .map(|l| l.trim_matches(|c: char| c.is_control() || c.is_whitespace()))
                    .filter(|l| !l.is_empty())
                    .collect();
                if lines.is_empty() {
                    self.line("reply", "(no reply)");
                }
                for l in lines.iter().take(14) {
                    self.line("reply", *l);
                }
                if lines.len() > 14 {
                    self.line("reply", format!("… {} more lines", lines.len() - 14));
                }
            }
            Err(e) => self.line("error", e.to_string()),
        }
        reply
    }

    async fn send(&self, command: &str) -> anyhow::Result<()> {
        self.line("send", command.trim());
        printer::send(self.ip, command.as_bytes()).await
    }

    fn link(&self, topic: Topic) -> Vec<Link> {
        topic.links(self.zt)
    }
}

fn shown(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "(not on this printer)".into(),
        serde_json::Value::String(s) => s.clone(),
        other => {
            let s = other.to_string();
            if s.len() > 160 {
                format!(
                    "{}…",
                    &s[..s.char_indices().nth(160).map_or(s.len(), |(i, _)| i)]
                )
            } else {
                s
            }
        }
    }
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").trim().to_string()
}

pub async fn run(
    ip: &str,
    checks: &[Check],
    ctx: &Context,
    log: &(dyn Fn(LogLine) + Sync),
) -> Vec<Finding> {
    let mut s = Session {
        ip,
        log,
        findings: vec![],
        zt: false,
    };
    s.line("step", "Connection");
    let Some(info) = connection(&mut s, ctx).await else {
        return s.findings;
    };
    s.zt = info.model.starts_with("ZT");

    for c in catalogue() {
        if !checks.contains(&c.id) {
            continue;
        }
        s.line("step", c.label);
        let done = match c.id {
            Check::Alerts => alerts(&mut s).await,
            Check::HostStatus => host_status(&mut s).await,
            Check::Printhead => printhead(&mut s).await,
            Check::Media => media(&mut s, &info).await,
            Check::Sensors => sensors(&mut s).await,
            Check::Profile => profile(&mut s, ctx, &info).await,
            Check::Memory => memory(&mut s).await,
            Check::Network => network(&mut s).await,
            Check::Weblink => weblink(&mut s).await,
            Check::Clock => clock(&mut s, ctx).await,
            Check::Wear => wear(&mut s).await,
            Check::Restarts => restarts(&mut s).await,
            Check::FeedTest => feed_test(&mut s).await,
            Check::CalibrationTest => calibration_test(&mut s).await,
            Check::ElementTest => element_test(&mut s, &info).await,
            Check::SensorProfile => sensor_profile(&mut s).await,
            Check::Connection => Ok(()),
        };
        if let Err(e) = done {
            s.find(
                c.id,
                Severity::Warning,
                format!("{}: couldn't finish", c.label),
                e.to_string(),
            );
        }
    }

    let count = |sev| s.findings.iter().filter(|f| f.severity == sev).count();
    let summary = format!(
        "Done: {} errors, {} warnings",
        count(Severity::Error),
        count(Severity::Warning)
    );
    s.line("step", summary);
    s.findings
}

async fn connection(s: &mut Session<'_>, ctx: &Context) -> Option<Info> {
    let started = Instant::now();
    let got = s
        .get(&[
            "device.product_name",
            "device.unique_id",
            "appl.name",
            "appl.link_os_version",
            "head.resolution.in_dpi",
            "ip.hostname",
        ])
        .await;
    let ms = started.elapsed().as_millis();
    let v = match got {
        Ok(v) => v,
        Err(e) => {
            let ip = s.ip;
            s.find(
                Check::Connection,
                Severity::Error,
                "Can't reach the printer",
                format!("{e}. Check that it's on and connected, and that {ip} is its address."),
            );
            return None;
        }
    };
    let info = Info {
        model: text(&v, "device.product_name"),
        serial: text(&v, "device.unique_id"),
        dpi: text(&v, "head.resolution.in_dpi").parse().unwrap_or(203),
        ip_hostname: v["ip.hostname"].is_string(),
    };
    let firmware = text(&v, "appl.name");
    let about = format!(
        "{} {}, firmware {} (Link-OS {})",
        info.model,
        info.serial,
        firmware,
        text(&v, "appl.link_os_version")
    );
    if ms > 2000 {
        s.find(
            Check::Connection,
            Severity::Warning,
            format!("Slow to answer ({ms} ms)"),
            format!("{about}. A busy network or weak Wi-Fi can make jobs slow or fail."),
        );
    } else {
        s.find(
            Check::Connection,
            Severity::Ok,
            format!("Connected in {ms} ms"),
            about,
        );
    }

    let folders = settings::Settings {
        folders: ctx.folders.clone(),
        ..Default::default()
    };
    if let Some(want) = folders
        .folder_for(Some(&info.model))
        .and_then(|f| f.firmware_version.clone())
        && want != firmware
    {
        s.find(
            Check::Connection,
            Severity::Warning,
            "Firmware differs from the Zebra folder's",
            format!(
                "The printer has {firmware}; its Zebra folder installs {want}. Apply a profile with the Zebra folder from Configure to update it."
            ),
        );
    }
    Some(info)
}

async fn alerts(s: &mut Session<'_>) -> anyhow::Result<()> {
    let reply = s.ask("~HQES", 1500, printer::hqes_complete).await?;
    let a = printer::alerts(&reply).ok_or_else(|| anyhow!("no reply to ~HQES"))?;
    let media_now = media_present(s).await.unwrap_or(false);

    for e in &a.errors {
        let (detail, topic): (&str, Option<Topic>) = match e.as_str() {
            "media out" if media_now => (
                "The printer still shows the error, but media is loaded now, so it can be cleared.",
                Some(Topic::MediaOut),
            ),
            "media out" => (
                "Load media and close the printhead. If media is loaded, the sensor may be dirty, out of position, or set for the wrong media type.",
                Some(Topic::MediaOut),
            ),
            "head open" => (
                "Close the printhead until it latches.",
                Some(Topic::HeadOpen),
            ),
            "ribbon out" => (
                "Load a ribbon, or check the ribbon sensor.",
                Some(Topic::RibbonOut),
            ),
            "printhead over temperature" | "motor over temperature" => (
                "The printer pauses to cool down. Large jobs and direct sunlight are common causes.",
                Some(Topic::OverTemp),
            ),
            "bad printhead element" => (
                "Dead printhead dots leave white lines down the labels. The printhead needs replacing.",
                Some(Topic::ReplaceHead),
            ),
            "printhead detection error" | "printhead thermistor open" => (
                "The printer can't read its printhead. Turn it off and on; if it stays, the printhead or its cable needs service.",
                Some(Topic::Shutdown),
            ),
            "invalid firmware configuration" => (
                "The firmware doesn't match this printer's hardware. Reload the right firmware.",
                None,
            ),
            "cutter fault" => ("Check the cutter for jammed media.", None),
            _ => (
                "See Zebra's ~HQ reference for this flag.",
                Some(Topic::Hqes),
            ),
        };
        let links = topic.map(|t| s.link(t)).unwrap_or_default();
        let f = s
            .find(Check::Alerts, Severity::Error, capitalise(e), detail)
            .links(links);
        if e == "media out" && media_now {
            f.fix(Fix::ClearError);
        }
    }
    if a.paused {
        let f = s.find(
            Check::Alerts,
            Severity::Warning,
            "Paused",
            "The printer won't print until it's resumed.",
        );
        if a.errors.is_empty() {
            f.fix(Fix::Resume);
        }
    }
    for w in &a.warnings {
        match w.as_str() {
            "needs media calibration" => {
                s.find(
                    Check::Alerts,
                    Severity::Warning,
                    "Needs media calibration",
                    "The printer asks for a calibration, usually after a media change.",
                )
                .fix(Fix::Calibrate);
            }
            "clean printhead" => {
                let links = s.link(Topic::CleanHead);
                s.find(
                    Check::Alerts,
                    Severity::Warning,
                    "Clean the printhead",
                    "The maintenance reminder for cleaning is due.",
                )
                .links(links);
            }
            "replace printhead" => {
                let links = s.link(Topic::ReplaceHead);
                s.find(
                    Check::Alerts,
                    Severity::Warning,
                    "Replace the printhead",
                    "The printhead has reached its replacement reminder.",
                )
                .links(links);
            }
            other => {
                let links = s.link(Topic::Hqes);
                s.find(Check::Alerts, Severity::Warning, capitalise(other), "")
                    .links(links);
            }
        }
    }
    if a.errors.is_empty() && a.warnings.is_empty() && !a.paused {
        s.find(Check::Alerts, Severity::Ok, "No errors or warnings", "");
    }
    Ok(())
}

// the transmissive sensor sees straight through when there's no media, whichever sensor is
// selected
async fn media_present(s: &Session<'_>) -> anyhow::Result<bool> {
    let v = s
        .get(&[
            "sensor.gap.cur",
            "sensor.gap.ppr_out_thold",
            "sensor.paper_supply",
        ])
        .await?;
    let num = |k: &str| text(&v, k).parse::<u32>().ok();
    let see_through = matches!(
        (num("sensor.gap.cur"), num("sensor.gap.ppr_out_thold")),
        (Some(cur), Some(thold)) if cur >= thold
    );
    Ok(!see_through && text(&v, "sensor.paper_supply") != "out")
}

async fn host_status(s: &mut Session<'_>) -> anyhow::Result<()> {
    let reply = s.ask("~HS", 1000, printer::hs_complete).await?;
    let h = printer::parse_hs(&reply).ok_or_else(|| anyhow!("couldn't read the ~HS reply"))?;
    let before = s.findings.len();
    if h.corrupt_ram {
        s.find(
            Check::HostStatus,
            Severity::Error,
            "Saved settings were lost",
            "The printer reports corrupt configuration memory. Check its settings, apply its profile again, and restart it.",
        )
        .fix(Fix::Restart);
    }
    if h.over_temperature {
        let links = s.link(Topic::OverTemp);
        s.find(
            Check::HostStatus,
            Severity::Error,
            "Printhead too hot",
            "Let it cool, and keep it out of direct sunlight.",
        )
        .links(links);
    }
    if h.under_temperature {
        s.find(
            Check::HostStatus,
            Severity::Warning,
            "Printhead too cold",
            "Printing may be faint until it warms up.",
        );
    }
    if h.comm_diagnostics {
        s.find(
            Check::HostStatus,
            Severity::Warning,
            "Communications diagnostics mode is on",
            "The printer prints what it receives as hex instead of printing labels.",
        )
        .fix(Fix::StopCommDiagnostics);
    }
    if h.buffer_full || h.partial_format {
        let what = if h.buffer_full {
            "The receive buffer is full"
        } else {
            "A label format arrived only partly"
        };
        s.find(
            Check::HostStatus,
            Severity::Warning,
            what,
            "Jobs may be stuck. Cancelling them clears the buffer.",
        )
        .fix(Fix::CancelJobs);
    } else if h.formats > 0 {
        s.find(
            Check::HostStatus,
            Severity::Info,
            format!("{} label formats waiting", h.formats),
            if h.paused {
                "They print when the printer is resumed."
            } else {
                "They're waiting in the receive buffer."
            },
        )
        .fix(Fix::CancelJobs);
    }
    if h.label_waiting {
        s.find(
            Check::HostStatus,
            Severity::Info,
            "A label is waiting to be taken",
            "In peel-off mode the next label prints when this one is removed.",
        );
    }
    if s.findings.len() == before {
        s.find(
            Check::HostStatus,
            Severity::Ok,
            "Printer state is normal",
            format!(
                "Label length {} dots, nothing stuck in the buffer.",
                h.label_length
            ),
        );
    }
    Ok(())
}

async fn printhead(s: &mut Session<'_>) -> anyhow::Result<()> {
    let hd = s.ask("~HD", 800, |_| false).await?;
    let pairs = printer::parse_pairs(&hd, " = ");
    let jt = s.ask("~HQJT", 1000, printer::hqjt_complete).await?;
    let test = printer::parse_hqjt(&jt);
    let temp = pairs
        .get("Head Temp")
        .map(|t| format!(" The printhead is at {} °C.", t.trim_start_matches('0')))
        .unwrap_or_default();
    let failed_test = pairs.get("Head Test").is_some_and(|t| t.contains("Fail"));

    match test {
        Some(t) if t.failures > 0 || failed_test => {
            let mut links = s.link(Topic::ReplaceHead);
            links.extend(Topic::ElementOut.links(s.zt));
            s.find(
                Check::Printhead,
                Severity::Error,
                format!("{} printhead dots failed", t.failures.max(1)),
                format!(
                    "Dead dots leave white lines down the labels, and barcodes can stop scanning. The printhead needs replacing.{temp}"
                ),
            )
            .links(links);
        }
        Some(t) if t.last > 0 => {
            s.find(
                Check::Printhead,
                Severity::Ok,
                "Printhead passed its element test",
                format!(
                    "Dots {} to {} were tested, none failed.{temp}",
                    t.first, t.last
                ),
            );
        }
        _ => {
            s.find(
                Check::Printhead,
                Severity::Info,
                "The printhead element test hasn't run",
                format!("Pick \"Printhead element test\" to run it; it prints one label.{temp}"),
            );
        }
    }
    Ok(())
}

async fn media(s: &mut Session<'_>, info: &Info) -> anyhow::Result<()> {
    let v = s
        .get(&[
            "ezpl.media_type",
            "media.sense_mode",
            "device.sensor_select",
            "ezpl.print_width",
        ])
        .await?;
    s.line("send", "getvar allcv (for the print width range)");
    let max = printer::print_width_max(s.ip).await?;
    s.line(
        "reply",
        format!("ezpl.print_width accepts up to {max} dots"),
    );

    let (kind, sense, sensor) = (
        text(&v, "ezpl.media_type"),
        text(&v, "media.sense_mode"),
        text(&v, "device.sensor_select"),
    );
    let width: u32 = text(&v, "ezpl.print_width").parse().unwrap_or(0);
    let before = s.findings.len();

    if kind == "mark" && (sensor != "reflective" || sense != "bar") {
        s.find(
            Check::Media,
            Severity::Warning,
            "Black mark media, but not black mark sensing",
            format!("The sensor is {sensor} and sensing is {sense}; black marks need the reflective sensor."),
        )
        .fix(Fix::MatchSensor { mark: true });
    }
    if kind == "gap/notch" && (sensor != "transmissive" || sense != "gap") {
        s.find(
            Check::Media,
            Severity::Warning,
            "Gap/notch media, but not gap sensing",
            format!("The sensor is {sensor} and sensing is {sense}; gaps and notches need the transmissive sensor."),
        )
        .fix(Fix::MatchSensor { mark: false });
    }
    if kind == "auto_detect" {
        s.find(
            Check::Media,
            Severity::Info,
            "Media type is auto-detect",
            "Calibration then decides between gaps and black marks itself, and can choose gaps on black-mark media such as wristbands. Pick the method on the Printer page when calibrating.",
        );
    }
    let nominal = max / info.dpi.max(1) * info.dpi;
    if nominal > 0 && width > nominal {
        s.find(
            Check::Media,
            Severity::Warning,
            format!("Print width is wider than the printhead's {} inches", nominal / info.dpi),
            format!("It's {width} dots; {nominal} is the full width in whole inches. Anything past that prints off the edge."),
        )
        .fix(Fix::PrintWidth { dots: nominal });
    }
    if s.findings.len() == before {
        s.find(
            Check::Media,
            Severity::Ok,
            "Media settings agree",
            format!("{kind}, {sensor} sensor, {width} dots wide."),
        );
    }
    Ok(())
}

async fn sensors(s: &mut Session<'_>) -> anyhow::Result<()> {
    let v = s
        .get(&["sensor.gap.cur", "sensor.gap.ppr_out_thold", "head.latch"])
        .await?;
    let hh = s.ask("^XA^HH^XZ", 1500, |_| false).await?;
    let cal = [
        "WEB SENSOR",
        "MEDIA SENSOR",
        "MARK SENSOR",
        "MARK MED SENSOR",
    ]
    .iter()
    .filter_map(|label| config_value(&hh, label).map(|v| format!("{} {v}", label.to_lowercase())))
    .collect::<Vec<_>>()
    .join(", ");
    let levels = if cal.is_empty() {
        String::new()
    } else {
        format!(" Calibration levels: {cal}.")
    };
    let (cur, thold) = (
        text(&v, "sensor.gap.cur"),
        text(&v, "sensor.gap.ppr_out_thold"),
    );

    if text(&v, "head.latch") != "ok" {
        s.find(
            Check::Sensors,
            Severity::Warning,
            "The printhead is open",
            "Close it to check the media sensor.",
        );
    } else if !media_present(s).await? {
        let links = s.link(Topic::MediaOut);
        s.find(
            Check::Sensors,
            Severity::Warning,
            "No media detected",
            format!("The transmissive sensor reads {cur}, at or past its paper-out level of {thold}: no media, or the media isn't under the sensor.{levels}"),
        )
        .links(links);
    } else {
        s.find(
            Check::Sensors,
            Severity::Ok,
            "Media detected",
            format!("The transmissive sensor reads {cur}; paper out is {thold}.{levels}"),
        );
    }
    Ok(())
}

// "  044                 WEB SENSOR" -> "044", from the configuration label text ^HH returns
fn config_value(hh: &str, label: &str) -> Option<String> {
    hh.lines().find_map(|l| {
        let l = l.trim_end();
        let value = l.strip_suffix(label)?;
        // the label is set off from the value by spaces, so "MARK SENSOR" doesn't match
        // "MARK MED SENSOR"'s line
        if !value.ends_with("  ") {
            return None;
        }
        Some(value.trim().to_string())
    })
}

async fn profile(s: &mut Session<'_>, ctx: &Context, info: &Info) -> anyhow::Result<()> {
    let Some(p) = &ctx.profile else {
        s.find(
            Check::Profile,
            Severity::Info,
            "No profile to compare with",
            "Choose one above to check the printer still has its settings.",
        );
        return Ok(());
    };
    if !model::fits(&p.models, Some(&info.model)) {
        s.find(
            Check::Profile,
            Severity::Info,
            format!("{} isn't for a {}", p.name, info.model),
            "",
        );
        return Ok(());
    }
    let max_dots = match &p.media {
        Some(_) => printer::print_width_max(s.ip).await?,
        None => 0,
    };
    let target = setup::Target {
        serial: info.serial.clone(),
        dpi: info.dpi,
        max_dots,
        ip_hostname: info.ip_hostname,
    };
    let want = setup::settings(p, Some(&target)).map_err(|e| anyhow!(e))?;
    let keys: Vec<&str> = want.iter().map(|w| w.key.as_str()).collect();
    let got = s.get(&keys).await?;
    let differ: Vec<(String, String, String)> = want
        .iter()
        .filter(|w| {
            !got[w.key.as_str()]
                .as_str()
                .is_some_and(|g| worker::same_value(g, &w.value))
        })
        .map(|w| (w.key.clone(), shown(&got[w.key.as_str()]), w.value.clone()))
        .collect();
    if differ.is_empty() {
        s.find(
            Check::Profile,
            Severity::Ok,
            format!("Matches the {} profile", p.name),
            format!("All {} of its settings are in place.", want.len()),
        );
    } else {
        let detail = differ
            .iter()
            .map(|(k, have, want)| format!("{k}: has {have}, profile sets {want}"))
            .collect::<Vec<_>>()
            .join("\n");
        let settings = differ
            .into_iter()
            .map(|(k, _, w)| (k, w))
            .collect::<Vec<_>>();
        s.find(
            Check::Profile,
            Severity::Warning,
            format!(
                "{} settings differ from the {} profile",
                settings.len(),
                p.name
            ),
            detail,
        )
        .fix(Fix::ApplySettings { settings });
    }
    Ok(())
}

async fn memory(s: &mut Session<'_>) -> anyhow::Result<()> {
    let v = s
        .get(&[
            "memory.ram_size",
            "memory.ram_free",
            "memory.flash_size",
            "memory.flash_free",
        ])
        .await?;
    // "8388608 Bytes" or "66932736 Bytes Free"
    let bytes = |k: &str| -> Option<f64> { text(&v, k).split_whitespace().next()?.parse().ok() };
    let share = |free: &str, size: &str| match (bytes(free), bytes(size)) {
        (Some(f), Some(t)) if t > 0.0 => Some((f / t * 100.0, t / 1_048_576.0)),
        _ => None,
    };
    let (Some((ram, ram_mb)), Some((flash, flash_mb))) = (
        share("memory.ram_free", "memory.ram_size"),
        share("memory.flash_free", "memory.flash_size"),
    ) else {
        s.find(
            Check::Memory,
            Severity::Info,
            "This printer doesn't report its memory",
            "",
        );
        return Ok(());
    };
    let summary = format!(
        "RAM {ram:.0}% free of {ram_mb:.0} MB, flash {flash:.0}% free of {flash_mb:.0} MB."
    );
    if ram < 10.0 || flash < 10.0 {
        let links = s.link(Topic::OutOfMemory);
        s.find(
            Check::Memory,
            Severity::Warning,
            if flash < 10.0 {
                "Flash is nearly full"
            } else {
                "RAM is nearly full"
            },
            format!("{summary} Remove unused fonts, graphics and formats."),
        )
        .links(links);
    } else {
        s.find(Check::Memory, Severity::Ok, "Plenty of memory", summary);
    }
    Ok(())
}

async fn network(s: &mut Session<'_>) -> anyhow::Result<()> {
    let v = s
        .get(&[
            "interface.network.active.printserver",
            "interface.network.active.ip_addr",
            "interface.network.active.protocol",
            "interface.network.active.cable_type",
            "interface.network.active.dhcp_received_host_name",
            "interface.network.active.rx_errors",
            "interface.network.active.tx_errors",
            "ip.primary_network",
            "wlan.signal_strength",
            "ip.hostname",
            "device.friendly_name",
        ])
        .await?;
    let wireless = text(&v, "interface.network.active.printserver").contains("wireless");
    let before = s.findings.len();

    if wireless
        && let Ok(signal) = text(&v, "wlan.signal_strength").parse::<u32>()
        && signal < 40
    {
        s.find(
            Check::Network,
            Severity::Warning,
            format!("Weak Wi-Fi signal ({signal}%)"),
            "A weak signal drops connections and slows jobs. Move the printer or access point closer, or use a cable.",
        );
    }
    let rx: u64 = text(&v, "interface.network.active.rx_errors")
        .parse()
        .unwrap_or(0);
    let tx: u64 = text(&v, "interface.network.active.tx_errors")
        .parse()
        .unwrap_or(0);
    if rx + tx > 0 {
        s.find(
            Check::Network,
            Severity::Warning,
            format!("Network errors: {rx} received, {tx} sent"),
            "Errors point to a bad cable, a faulty switch port, or interference on Wi-Fi.",
        );
    }
    if wireless && text(&v, "ip.primary_network") == "1" {
        s.find(
            Check::Network,
            Severity::Info,
            "Prefers wired, but is on Wi-Fi",
            "No cable is plugged in, or the wired port has no link.",
        );
    }
    let hostname = match v["ip.hostname"].as_str() {
        Some(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => text(&v, "device.friendly_name"),
    };
    let dhcp_name = text(&v, "interface.network.active.dhcp_received_host_name");
    if !dhcp_name.is_empty() && !hostname.is_empty() && !dhcp_name.eq_ignore_ascii_case(&hostname) {
        let links = s.link(Topic::Hostname76);
        s.find(
            Check::Network,
            Severity::Warning,
            "DHCP knows it by another name",
            format!("DHCP reports {dhcp_name}, but the printer's hostname is {hostname}, so its DNS name may not be the one you expect. On Link-OS 7.6 and later the hostname is ip.hostname."),
        )
        .links(links);
    }
    if s.findings.len() == before {
        s.find(
            Check::Network,
            Severity::Ok,
            "Network looks fine",
            format!(
                "{} over {}, address {} by {}.",
                hostname,
                text(&v, "interface.network.active.cable_type"),
                text(&v, "interface.network.active.ip_addr"),
                text(&v, "interface.network.active.protocol")
            ),
        );
    }
    Ok(())
}

async fn weblink(s: &mut Session<'_>) -> anyhow::Result<()> {
    let v = s
        .get(&[
            "weblink.ip.conn1.location",
            "weblink.ip.conn1.num_connections",
            "weblink.ip.conn2.location",
            "weblink.ip.conn2.num_connections",
        ])
        .await?;
    let mut configured = false;
    for n in [1, 2] {
        let location = text(&v, &format!("weblink.ip.conn{n}.location"));
        if location.is_empty() {
            continue;
        }
        configured = true;
        let connections: u32 = text(&v, &format!("weblink.ip.conn{n}.num_connections"))
            .parse()
            .unwrap_or(0);
        if connections > 0 {
            s.find(
                Check::Weblink,
                Severity::Ok,
                format!("Weblink {n} is connected"),
                location,
            );
            continue;
        }
        s.line("send", "file.dir E: (for the weblink certificates)");
        let files = printer::files(s.ip).await?;
        let missing: Vec<String> = [format!("WEBLINK{n}_CA.NRD"), format!("WEBLINK{n}_CERT.NRD")]
            .into_iter()
            .filter(|f| !files.contains_key(f))
            .collect();
        if missing.is_empty() {
            s.find(
                Check::Weblink,
                Severity::Warning,
                format!("Weblink {n} isn't connected"),
                format!("It's set to {location} but has no connection. The server may be unreachable or its name may not resolve, or the printer's clock may be wrong, which breaks the secure connection."),
            );
        } else {
            s.find(
                Check::Weblink,
                Severity::Error,
                format!("Weblink {n} isn't connected, and its certificates are missing"),
                format!("It's set to {location}, but {} isn't on E:. Apply the profile with the Zebra folder again from Configure.", missing.join(" and ")),
            );
        }
    }
    if !configured {
        s.find(
            Check::Weblink,
            Severity::Info,
            "No weblink server is set",
            "",
        );
    }
    Ok(())
}

async fn clock(s: &mut Session<'_>, ctx: &Context) -> anyhow::Result<()> {
    let v = s
        .get(&["rtc.date", "rtc.time", "rtc.timezone", "ip.ntp.enable"])
        .await?;
    let (date, time, tz) = (
        text(&v, "rtc.date"),
        text(&v, "rtc.time"),
        text(&v, "rtc.timezone"),
    );
    let Some(printer_secs) = parse_printer_time(&date, &time) else {
        s.find(
            Check::Clock,
            Severity::Info,
            "This printer doesn't report its clock",
            "",
        );
        return Ok(());
    };
    let offset = printer_offset(&tz, ctx.pc_offset_minutes).unwrap_or(ctx.pc_offset_minutes);
    let expected = now_utc() + i64::from(offset) * 60;
    let off_by = printer_secs - expected;
    let ntp = if text(&v, "ip.ntp.enable") == "on" {
        "NTP is on"
    } else {
        "NTP is off"
    };
    if off_by.abs() > 300 {
        s.find(
            Check::Clock,
            Severity::Warning,
            format!(
                "Clock is {} {}",
                if off_by > 0 { "fast by" } else { "slow by" },
                human_span(off_by.unsigned_abs())
            ),
            format!(
                "It says {date} {time} ({tz}); it should be {}. A wrong clock breaks weblink's secure connection. {ntp}.",
                format_printer_time(expected).join(" ")
            ),
        )
        .fix(Fix::SetClock {
            offset_minutes: offset,
        });
    } else {
        s.find(
            Check::Clock,
            Severity::Ok,
            "Clock is right",
            format!("{date} {time} ({tz}). {ntp}."),
        );
    }
    Ok(())
}

async fn wear(s: &mut Session<'_>) -> anyhow::Result<()> {
    let ph = s.ask("~HQPH", 800, |_| false).await?;
    let ma = s.ask("~HQMA", 800, |_| false).await?;
    let after = |text: &str, label: &str| -> Option<String> {
        text.lines()
            .find_map(|l| l.split_once(label).map(|(_, v)| v.trim().to_string()))
    };
    let cleaned = after(&ph, "LAST CLEANED:").and_then(|d| printer::parse_distance(&d));
    // the first entry of the head life history is the printhead in the printer now
    let head = after(&ph, "1:").and_then(|d| printer::parse_distance(&d));
    let rated = after(&ma, "HEAD REPLACEMENT INTERVAL:").and_then(|d| printer::parse_distance(&d));
    let reminders = after(&ma, "PRINT CLEANING ALERT:").is_some_and(|a| a.starts_with('Y'));

    if let (Some(head), Some(rated)) = (head, rated)
        && rated > 0.0
        && head / rated >= 0.9
    {
        let links = s.link(Topic::ReplaceHead);
        s.find(
            Check::Wear,
            Severity::Warning,
            "The printhead is near its rated life",
            format!(
                "It has printed {} of the {} it's rated for. Plan to replace it.",
                distance(head),
                distance(rated)
            ),
        )
        .links(links);
    }
    if let Some(cleaned) = cleaned {
        let links = s.link(Topic::CleanHead);
        let head = head
            .map(|h| format!(" This printhead has printed {} in all.", distance(h)))
            .unwrap_or_default();
        s.find(
            Check::Wear,
            Severity::Info,
            format!("{} printed since the printhead was cleaned", distance(cleaned)),
            format!(
                "Zebra recommends cleaning it with every new roll of media, or of ribbon on thermal transfer printers.{head}{}",
                if reminders { "" } else { " Cleaning reminders are off." }
            ),
        )
        .links(links);
    } else {
        s.find(
            Check::Wear,
            Severity::Info,
            "This printer doesn't report printhead wear",
            "",
        );
    }
    Ok(())
}

async fn restarts(s: &mut Session<'_>) -> anyhow::Result<()> {
    let reply = s
        .ask("! U1 getvar \"log.reboot.report\"\r\n", 800, |t| {
            t.matches('"').count() >= 2
        })
        .await?;
    let reasons: Vec<&str> = reply
        .lines()
        .map(|l| l.trim().trim_matches('"').trim())
        .filter(|l| !l.is_empty())
        .collect();
    if reasons.is_empty() || reasons == ["?"] {
        s.find(
            Check::Restarts,
            Severity::Info,
            "This printer doesn't keep a restart log",
            "",
        );
        return Ok(());
    }
    s.find(
        Check::Restarts,
        Severity::Info,
        format!("Last restart: {}", reasons[0]),
        format!(
            "Most recent first: {}. \"Off key\" is the power button and \"device.reset command\" a restart sent to it.",
            reasons.iter().take(6).copied().collect::<Vec<_>>().join(", ")
        ),
    );
    Ok(())
}

// the tests that feed media only run when there's media, the head is closed and nothing is wrong
async fn ready_to_feed(s: &mut Session<'_>, check: Check) -> anyhow::Result<bool> {
    s.line("send", "status check before feeding (~HQES, sensors)");
    let h = printer::health(s.ip).await?;
    let problem = if !h.head_closed {
        Some("the printhead is open".to_string())
    } else if !h.media {
        Some("no media is detected".to_string())
    } else if !h.errors.is_empty() {
        Some(format!(
            "the printer has an error ({})",
            h.errors.join(", ")
        ))
    } else if h.paused {
        Some("the printer is paused".to_string())
    } else {
        None
    };
    let Some(problem) = problem else {
        return Ok(true);
    };
    let f = s.find(
        check,
        Severity::Warning,
        "Skipped",
        format!("Not run because {problem}."),
    );
    if h.paused && h.errors.is_empty() && h.media && h.head_closed {
        f.fix(Fix::Resume);
    }
    Ok(false)
}

async fn feed_test(s: &mut Session<'_>) -> anyhow::Result<()> {
    if !ready_to_feed(s, Check::FeedTest).await? {
        return Ok(());
    }
    s.send("~PH").await?;
    let h = printer::wait_idle(s.ip, 20).await?;
    if h.errors.is_empty() {
        s.find(
            Check::FeedTest,
            Severity::Ok,
            "Fed a label cleanly",
            "The motor, platen roller and media path work.",
        );
    } else {
        let links = s.link(Topic::MediaOut);
        s.find(
            Check::FeedTest,
            Severity::Error,
            format!("Feeding failed: {}", h.errors.join(", ")),
            "Media may have run out or jammed, or the platen roller may not be gripping it.",
        )
        .links(links)
        .fix(Fix::ClearError);
    }
    Ok(())
}

async fn calibration_test(s: &mut Session<'_>) -> anyhow::Result<()> {
    if !ready_to_feed(s, Check::CalibrationTest).await? {
        return Ok(());
    }
    s.send("~JC").await?;
    let h = printer::wait_idle(s.ip, 60).await?;
    let mut links = s.link(Topic::SmartCal);
    links.extend(s.link(Topic::ManualCal));
    if !h.errors.is_empty() {
        s.find(
            Check::CalibrationTest,
            Severity::Error,
            format!("Calibration failed: {}", h.errors.join(", ")),
            "The sensor couldn't find the gaps or marks. Check the media type, the sensor's position and that it's clean, or calibrate manually.",
        )
        .links(links)
        .fix(Fix::ClearError);
        return Ok(());
    }
    let v = s
        .get(&[
            "zpl.label_length",
            "head.resolution.in_dpi",
            "ezpl.label_length_max",
            "ezpl.media_type",
        ])
        .await?;
    let num = |k: &str| text(&v, k).parse::<f64>().ok();
    match (num("zpl.label_length"), num("head.resolution.in_dpi")) {
        (Some(dots), Some(dpi)) if dpi > 0.0 => {
            let inches = dots / dpi;
            let max = num("ezpl.label_length_max").unwrap_or(39.0);
            if inches < 0.2 || inches >= max {
                s.find(
                    Check::CalibrationTest,
                    Severity::Warning,
                    format!("Measured label length looks wrong ({inches:.2} in)"),
                    format!(
                        "Media type is {}. Check it matches the media, then calibrate manually.",
                        text(&v, "ezpl.media_type")
                    ),
                )
                .links(links);
            } else {
                s.find(
                    Check::CalibrationTest,
                    Severity::Ok,
                    format!("Calibrated: labels measure {inches:.2} in"),
                    format!(
                        "{dots:.0} dots, media type {}.",
                        text(&v, "ezpl.media_type")
                    ),
                );
            }
        }
        _ => {
            s.find(
                Check::CalibrationTest,
                Severity::Ok,
                "Calibrated without errors",
                "",
            );
        }
    }
    Ok(())
}

async fn element_test(s: &mut Session<'_>, info: &Info) -> anyhow::Result<()> {
    if !ready_to_feed(s, Check::ElementTest).await? {
        return Ok(());
    }
    let v = s
        .get(&["print.troubleshooting_label_choices", "ezpl.print_width"])
        .await?;
    let before = v["print.troubleshooting_label_choices"]
        .as_str()
        .map(str::to_string);

    // ^JT0001 tests the printhead on the next label printed
    s.send("^XA^JT0001,N^XZ").await?;
    match &before {
        Some(_) => {
            s.send("! U1 setvar \"print.troubleshooting_label_choices\" \"element out\"\r\n")
                .await?;
            s.send("! U1 do \"print.troubleshooting_label_print\" \"\"\r\n")
                .await?;
        }
        // no troubleshooting labels on this printer: a solid bar shows dead dots just as well
        None => {
            let width: u32 = text(&v, "ezpl.print_width").parse().unwrap_or(info.dpi * 2);
            s.send(&format!(
                "^XA^FO0,20^GB{width},{h},{h}^FS^XZ",
                h = info.dpi / 2
            ))
            .await?;
        }
    }
    let waited = printer::wait_idle(s.ip, 30).await;
    let jt = s.ask("~HQJT", 1000, printer::hqjt_complete).await;
    let hd = s.ask("~HD", 800, |_| false).await;

    // put the interval back to off, its default, so a later ^JUS can't save "every label"
    s.send("^XA^JT0000^XZ").await?;
    if let Some(choice) = &before {
        s.send(&format!(
            "! U1 setvar \"print.troubleshooting_label_choices\" \"{choice}\"\r\n"
        ))
        .await?;
    }

    let h = waited?;
    let test = printer::parse_hqjt(&jt?);
    let pairs = printer::parse_pairs(&hd?, " = ");
    let failed = pairs.get("Head Test").is_some_and(|t| t.contains("Fail"));
    match test {
        _ if !h.errors.is_empty() => {
            s.find(
                Check::ElementTest,
                Severity::Error,
                format!("The test label didn't print: {}", h.errors.join(", ")),
                "",
            )
            .fix(Fix::ClearError);
        }
        Some(t) if t.failures > 0 || failed => {
            let mut links = s.link(Topic::ReplaceHead);
            links.extend(Topic::ElementOut.links(s.zt));
            s.find(
                Check::ElementTest,
                Severity::Error,
                format!("{} printhead dots failed", t.failures.max(1)),
                format!(
                    "Tested dots {} to {}. The printhead needs replacing.",
                    t.first, t.last
                ),
            )
            .links(links);
        }
        Some(t) if t.last > 0 => {
            s.find(
                Check::ElementTest,
                Severity::Ok,
                "No dead printhead dots",
                format!("Tested dots {} to {}. On the printed label, a thin white line through the pattern would also mean a dead dot.", t.first, t.last),
            );
        }
        _ => {
            s.find(
                Check::ElementTest,
                Severity::Info,
                "The printer didn't report a result",
                "Check the printed label: thin white lines through the pattern mean dead printhead dots.",
            );
        }
    }
    Ok(())
}

async fn sensor_profile(s: &mut Session<'_>) -> anyhow::Result<()> {
    if !ready_to_feed(s, Check::SensorProfile).await? {
        return Ok(());
    }
    s.send("~JG").await?;
    let h = printer::wait_idle(s.ip, 40).await?;
    let links = s.link(Topic::ManualCal);
    if h.errors.is_empty() {
        s.find(
            Check::SensorProfile,
            Severity::Info,
            "Printed a sensor profile",
            "The graph should dip sharply at every gap or mark. A flat line means the sensor can't see them: move or clean it, check the media type, or calibrate manually.",
        )
        .links(links);
    } else {
        s.find(
            Check::SensorProfile,
            Severity::Error,
            format!("The sensor profile stopped: {}", h.errors.join(", ")),
            "",
        )
        .links(links)
        .fix(Fix::ClearError);
    }
    Ok(())
}

pub async fn apply_fix(ip: &str, fix: &Fix) -> Result<String, String> {
    let failed = |e: anyhow::Error| e.to_string();
    let send = |data: String| async move { printer::send(ip, data.as_bytes()).await };
    match fix {
        Fix::Resume => {
            send("! U1 do \"device.unpause\" \"\"\r\n".into())
                .await
                .map_err(failed)?;
            Ok("Resumed".into())
        }
        Fix::ClearError => {
            worker::recover(ip, &|_| {})
                .await
                .map_err(|e| e.into_message())?;
            Ok("Cleared; the printer is paused until it's resumed".into())
        }
        Fix::CancelJobs => {
            send("~JA".into()).await.map_err(failed)?;
            Ok("Cancelled the waiting jobs".into())
        }
        Fix::Restart => {
            send("! U1 do \"device.reset\" \"\"\r\n".into())
                .await
                .map_err(failed)?;
            Ok("Restarting; it takes about a minute".into())
        }
        Fix::Calibrate => {
            send("~JC".into()).await.map_err(failed)?;
            let h = printer::wait_idle(ip, 60).await.map_err(failed)?;
            match h.errors.is_empty() {
                true => Ok("Calibrated".into()),
                false => Err(format!("calibration stopped with {}", h.errors.join(", "))),
            }
        }
        Fix::MatchSensor { mark } => {
            let (sense, sensor) = if *mark {
                ("bar", "reflective")
            } else {
                ("gap", "transmissive")
            };
            set(
                ip,
                &[
                    ("media.sense_mode", sense),
                    ("device.sensor_select", sensor),
                ],
            )
            .await?;
            Ok(format!("Now using the {sensor} sensor"))
        }
        Fix::PrintWidth { dots } => {
            set(ip, &[("ezpl.print_width", &dots.to_string())]).await?;
            Ok(format!("Print width is {dots} dots"))
        }
        Fix::SetClock { offset_minutes } => {
            let [date, time] = format_printer_time(now_utc() + i64::from(*offset_minutes) * 60);
            // the clock moves on while it's set, so only the date is compared afterwards
            send(format!(
                "! U1 setvar \"rtc.date\" \"{date}\"\r\n! U1 setvar \"rtc.time\" \"{time}\"\r\n"
            ))
            .await
            .map_err(failed)?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let got = printer::query(ip, &["rtc.date", "rtc.time"])
                .await
                .map_err(failed)?;
            if text(&got, "rtc.date") != date {
                return Err(format!("the printer kept {}", text(&got, "rtc.date")));
            }
            Ok(format!("Clock set to {date} {}", text(&got, "rtc.time")))
        }
        Fix::StopCommDiagnostics => {
            send("~JE".into()).await.map_err(failed)?;
            Ok("Diagnostics mode is off".into())
        }
        Fix::ApplySettings { settings } => {
            let pairs: Vec<(&str, &str)> = settings
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            set(ip, &pairs).await?;
            Ok(format!("Set {} settings", settings.len()))
        }
    }
}

// setvars, then reads them back
async fn set(ip: &str, pairs: &[(&str, &str)]) -> Result<(), String> {
    let lines: String = pairs
        .iter()
        .map(|(k, v)| format!("! U1 setvar \"{k}\" \"{v}\"\r\n"))
        .collect();
    printer::send(ip, lines.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let keys: Vec<&str> = pairs.iter().map(|(k, _)| *k).collect();
    let got = printer::query(ip, &keys).await.map_err(|e| e.to_string())?;
    let refused: Vec<&str> = pairs
        .iter()
        .filter(|(k, v)| !got[k].as_str().is_some_and(|g| worker::same_value(g, v)))
        .map(|(k, _)| *k)
        .collect();
    match refused.is_empty() {
        true => Ok(()),
        false => Err(format!("the printer didn't take {}", refused.join(", "))),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn distance(metres: f64) -> String {
    let feet = metres * 3.28084;
    if metres >= 1000.0 {
        format!("{:.1} km ({:.0} ft)", metres / 1000.0, feet)
    } else {
        format!("{metres:.0} m ({feet:.0} ft)")
    }
}

fn human_span(secs: u64) -> String {
    match secs {
        s if s < 3600 => format!("{} minutes", s / 60),
        s if s < 172_800 => format!("{:.1} hours", s as f64 / 3600.0),
        s => format!("{} days", s / 86_400),
    }
}

fn now_utc() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// minutes east of UTC that the printer's clock keeps, from its POSIX TZ string. daylight
// saving is taken to be in effect when this computer's offset is an hour ahead of the zone's
// standard time, which holds when both are in the same zone
fn printer_offset(tz: &str, pc_offset: i32) -> Option<i32> {
    let (west, daylight) = parse_tz(tz)?;
    let standard = -west;
    Some(if daylight && pc_offset == standard + 60 {
        standard + 60
    } else {
        standard
    })
}

// "EST5EDT,M3.2.0/2:00:00,M11.1.0/2:00:00" -> (300 minutes west, has daylight saving);
// "UTC00" -> (0, false); "CET-1CEST" -> (-60, true)
fn parse_tz(tz: &str) -> Option<(i32, bool)> {
    let tz = tz.trim();
    let rest = match tz.strip_prefix('<') {
        Some(quoted) => quoted.split_once('>')?.1,
        None => {
            let rest = tz.trim_start_matches(|c: char| c.is_ascii_alphabetic());
            if rest.len() == tz.len() {
                return None; // no zone name
            }
            rest
        }
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, ':' | '+' | '-')))
        .unwrap_or(rest.len());
    let offset = &rest[..end];
    let (sign, body) = match offset.chars().next()? {
        '-' => (-1, &offset[1..]),
        '+' => (1, &offset[1..]),
        _ => (1, offset),
    };
    let mut parts = body.split(':');
    let hours: i32 = parts.next()?.parse().ok()?;
    let minutes: i32 = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    let daylight = rest[end..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '<');
    Some((sign * (hours * 60 + minutes), daylight))
}

// "10-20-2026" and "04:50:06" -> seconds since 1970 as if the printer's local time were UTC
fn parse_printer_time(date: &str, time: &str) -> Option<i64> {
    let d: Vec<u32> = date
        .split('-')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let t: Vec<i64> = time
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [month, day, year] = d[..] else {
        return None;
    };
    let [h, m, sec] = t[..] else {
        return None;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(i64::from(year), month, day) * 86_400 + h * 3600 + m * 60 + sec)
}

// the reverse, as the rtc.date and rtc.time settings take it
fn format_printer_time(secs: i64) -> [String; 2] {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    [
        format!("{m:02}-{d:02}-{y:04}"),
        format!("{:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60),
    ]
}

// days since 1970-01-01, from Howard Hinnant's chrono-compatible date algorithms
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

// pages from Zebra's guides. ZT printers get their user guide; everything else the ZD421/ZD621
// guide, the closest to the ZD printers this app sets up
#[derive(Clone, Copy)]
enum Topic {
    MediaOut,
    HeadOpen,
    RibbonOut,
    OverTemp,
    Shutdown,
    OutOfMemory,
    CleanHead,
    ReplaceHead,
    SmartCal,
    ManualCal,
    ElementOut,
    Hostname76,
    Hqes,
}

impl Topic {
    fn links(self, zt: bool) -> Vec<Link> {
        let zd = |title: &'static str, url: &'static str| vec![Link { title, url }];
        let zt_guide = || {
            vec![Link {
                title: "ZT411/ZT421 user guide (see Diagnostics and Troubleshooting)",
                url: "https://docs.zebra.com/us/en/printers/industrial/zt411-zt421-ug/c-zt411-zt421-industrial-printer-user-guide.html",
            }]
        };
        match self {
            Topic::ElementOut => zd(
                "Zebra: Configuring printhead element out detection",
                "https://support.zebra.com/article/Configuring-Printhead-Element-Out-Detection?language=en_US",
            ),
            Topic::Hostname76 => zd(
                "Zebra: network naming changes in Link-OS 7.6",
                "https://support.zebra.com/article/000035294",
            ),
            Topic::Hqes => zd(
                "Zebra ZPL reference: ~HQ status flags",
                "https://docs.zebra.com/us/en/printers/software/zpl-pg/c-zpl-zpl-commands/r-zpl-hq.html",
            ),
            _ if zt => zt_guide(),
            Topic::MediaOut => zd(
                "ZD421/ZD621 user guide: Alert: Media Out",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/c-zd421-zd621-ug-troubleshooting/c-mlk-ug-resolving-alerts-and-errors/c-zd421-zd621-ug-alert-media-out.html",
            ),
            Topic::HeadOpen => zd(
                "ZD421/ZD621 user guide: Alert: Printhead Open",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/c-zd421-zd621-ug-troubleshooting/c-mlk-ug-resolving-alerts-and-errors/c-zd421-zd621-ug-alert-printhead-open.html",
            ),
            Topic::RibbonOut => zd(
                "ZD421/ZD621 user guide: Alert: Ribbon Out",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/troubleshooting/resolving-alerts-and-errors/alert-ribbon-out.html",
            ),
            Topic::OverTemp => zd(
                "ZD421/ZD621 user guide: Alert: Printhead Over Temp",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/c-zd421-zd621-ug-troubleshooting/c-mlk-ug-resolving-alerts-and-errors/c-zd421-zd621-ug-alert-printhead-over-temp.html",
            ),
            Topic::Shutdown => zd(
                "ZD421/ZD621 user guide: Alert: Printhead Shutdown",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/c-zd421-zd621-ug-troubleshooting/c-mlk-ug-resolving-alerts-and-errors/c-zd421-zd621-ug-alert-printhead-shutdown.html",
            ),
            Topic::OutOfMemory => zd(
                "ZD421/ZD621 user guide: Alert: Out of Memory",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/c-zd421-zd621-ug-troubleshooting/c-mlk-ug-resolving-alerts-and-errors/c-zd421-zd621-ug-alert-out-of-memory.html",
            ),
            Topic::CleanHead => zd(
                "ZD421/ZD621 user guide: Cleaning the Printhead",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/c-zd421-zd621-ug-maintenance/c-zd200t-ug-cleaning/t-zd620-zd420-ug-cleaning-the-printhead/t-zd421-zd621-ug-cleaning-the-printhead-zd421-zd621-direct-thermal-printers.html",
            ),
            Topic::ReplaceHead => zd(
                "ZD421/ZD621 user guide: Replacing the Printhead",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/maintenance/cleaning/replacing-the-printhead/replacing-the-printhead-zd421-zd621-direct-thermal-printer-models.html",
            ),
            Topic::SmartCal => zd(
                "ZD421/ZD621 user guide: Running a SmartCal Media Calibration",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/setup/running-a-smartcal-media-calibration.html",
            ),
            Topic::ManualCal => zd(
                "ZD421/ZD621 user guide: Manually Calibrating Media",
                "https://docs.zebra.com/us/en/printers/desktop/zd421-and-zd621-desktop-printers-user-guide/tools/activating-advanced-mode/manually-calibrating-media.html",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_zones_parse_like_posix() {
        assert_eq!(
            parse_tz("EST5EDT,M3.2.0/2:00:00,M11.1.0/2:00:00"),
            Some((300, true))
        );
        assert_eq!(parse_tz("UTC00"), Some((0, false)));
        assert_eq!(parse_tz("CET-1CEST,M3.5.0,M10.5.0/3"), Some((-60, true)));
        assert_eq!(parse_tz("<+0530>-5:30"), Some((-330, false)));
        assert_eq!(parse_tz("5"), None);
        assert_eq!(parse_tz(""), None);
    }

    #[test]
    fn printer_offset_follows_daylight_saving_in_the_same_zone() {
        let eastern = "EST5EDT,M3.2.0/2:00:00,M11.1.0/2:00:00";
        assert_eq!(printer_offset(eastern, -240), Some(-240)); // summer on this computer
        assert_eq!(printer_offset(eastern, -300), Some(-300)); // winter
        assert_eq!(printer_offset(eastern, 60), Some(-300)); // computer elsewhere: standard time
        assert_eq!(printer_offset("UTC00", -240), Some(0));
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 1, 1), 10_957);
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        for days in [-1, 0, 59, 60, 10_957, 20_746, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        let secs = parse_printer_time("10-20-2026", "04:50:06").unwrap();
        assert_eq!(
            format_printer_time(secs),
            ["10-20-2026".to_string(), "04:50:06".to_string()]
        );
        assert_eq!(parse_printer_time("13-01-2026", "00:00:00"), None);
        assert_eq!(parse_printer_time("", ""), None);
    }

    #[test]
    fn reads_values_off_the_configuration_label() {
        let hh = "  044                 WEB SENSOR        \r\n  047                 MARK SENSOR       \r\n  004                 MARK MED SENSOR   \r\n";
        assert_eq!(config_value(hh, "WEB SENSOR").as_deref(), Some("044"));
        assert_eq!(config_value(hh, "MARK SENSOR").as_deref(), Some("047"));
        assert_eq!(config_value(hh, "MARK MED SENSOR").as_deref(), Some("004"));
        assert_eq!(config_value(hh, "TAKE LABEL"), None);
    }

    #[test]
    fn fixes_round_trip_through_the_frontend() {
        for fix in [
            Fix::Resume,
            Fix::MatchSensor { mark: true },
            Fix::PrintWidth { dots: 406 },
            Fix::SetClock {
                offset_minutes: -240,
            },
            Fix::ApplySettings {
                settings: vec![("print.tone".into(), "25.0".into())],
            },
        ] {
            let json = serde_json::to_string(&fix).unwrap();
            assert_eq!(serde_json::from_str::<Fix>(&json).unwrap(), fix);
        }
        assert_eq!(
            serde_json::to_value(Fix::Resume).unwrap(),
            serde_json::json!({"kind": "resume"})
        );
    }

    #[test]
    fn every_check_but_connection_can_be_picked() {
        let ids: Vec<Check> = catalogue().iter().map(|c| c.id).collect();
        assert!(!ids.contains(&Check::Connection));
        assert_eq!(ids.len(), 16);
        assert!(catalogue().iter().filter(|c| c.feeds).all(|c| matches!(
            c.id,
            Check::FeedTest | Check::CalibrationTest | Check::ElementTest | Check::SensorProfile
        )));
    }

    #[test]
    fn spans_and_distances_read_naturally() {
        assert_eq!(human_span(600), "10 minutes");
        assert_eq!(human_span(5400), "1.5 hours");
        assert_eq!(human_span(3 * 86_400), "3 days");
        assert_eq!(distance(1112.6), "1.1 km (3650 ft)");
        assert_eq!(distance(50.0), "50 m (164 ft)");
    }
}
