use crate::{
    AppState,
    model::{Printer, Profile, Status},
    printer,
    recent::Recent,
    settings, setup, usb,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    net::IpAddr,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

pub enum ApplyError {
    Unreachable(String), // retry
    Mismatch(String),    // printer answered but didn't take the setting: don't retry
    Unverified(String), // printer stopped answering partway through, e.g. it joined another network: don't retry
}

impl ApplyError {
    pub fn into_message(self) -> String {
        match self {
            ApplyError::Unreachable(m) | ApplyError::Mismatch(m) | ApplyError::Unverified(m) => m,
        }
    }
}

// change a printer and send the whole row to the GUI
pub fn update(app: &AppHandle, ip: &str, f: impl FnOnce(&mut Printer)) {
    let state = app.state::<AppState>();
    if let Some(p) = state.printers.lock().unwrap().get_mut(ip) {
        f(p);
        let _ = app.emit("printer-update", p.clone());
    }
    crate::tray::update(app);
}

pub fn set_status(app: &AppHandle, ip: &str, status: Status) {
    update(app, ip, |p| p.status = status);
}

#[derive(Serialize, Clone)]
struct Moved {
    from: String,
    printer: Printer,
}

// the printer answered at another address after a restart, so it's listed there from now on
fn moved(app: &AppHandle, from: &str, to: &str) {
    let state = app.state::<AppState>();
    let p = {
        let mut printers = state.printers.lock().unwrap();
        let Some(mut p) = printers.remove(from) else {
            return;
        };
        p.ip = to.to_string();
        printers.insert(to.to_string(), p.clone());
        p
    };
    {
        let mut tasks = state.tasks.lock().unwrap();
        if let Some(task) = tasks.remove(from) {
            tasks.insert(to.to_string(), task);
        }
    }
    let _ = app.emit(
        "printer-moved",
        Moved {
            from: from.to_string(),
            printer: p,
        },
    );
    crate::tray::update(app);
}

// an IP or hostname the printer could be reached at; an interface that's down reports 0.0.0.0
fn usable_address(addr: &str) -> bool {
    let addr = addr.trim();
    !addr.is_empty() && addr != "0.0.0.0"
}

// where a printer may answer besides the address it's listed under: IPs it had and hostnames
// it may have, newest first. only an answer with its serial counts, since DHCP can hand an old
// address to another printer, and the app never looks anywhere else
#[derive(Default)]
struct Whereabouts {
    serial: String,
    ip_addr: Option<String>,         // where it last answered
    hostname_before: Option<String>, // what it was called when this apply first read it
    places: Vec<String>,
}

impl Whereabouts {
    // what's known before reading the printer: its serial and model from when it was added, the
    // IP it last answered at, where the Printer and Diagnose pages found it, and the names it may
    // have, including the profile's
    fn new(printer: Option<&Printer>, recent: &[Recent], profile: &Profile) -> Self {
        let mut w = Whereabouts {
            serial: printer.and_then(|p| p.serial.clone()).unwrap_or_default(),
            ip_addr: printer.and_then(|p| p.ip_addr.clone()),
            ..Default::default()
        };
        if w.serial.is_empty() {
            return w; // nothing to tell it apart by
        }
        let mut places: Vec<String> = w.ip_addr.iter().cloned().collect();
        for r in recent
            .iter()
            .filter(|r| r.serial.as_deref() == Some(w.serial.as_str()))
        {
            places.extend(
                [
                    r.ip_addr.clone(),
                    Some(r.address.clone()),
                    r.hostname.clone(),
                ]
                .into_iter()
                .flatten(),
            );
        }
        let named: Vec<String> = profile
            .hostname
            .iter()
            .map(|h| h.replace("{serial}", &w.serial))
            .collect();
        let model = printer.and_then(|p| p.model.as_deref()).unwrap_or("");
        places.extend(restart_names(&named, model, &w.serial, &[]));
        w.learn(places);
        w
    }

    // what reading the printer showed: where it is now, and the names it may have after a restart
    fn read(&mut self, plan: &Plan) {
        if !plan.serial.is_empty() {
            self.serial = plan.serial.clone();
        }
        if plan.ip_addr.is_some() {
            self.ip_addr = plan.ip_addr.clone();
        }
        if self.hostname_before.is_none() {
            self.hostname_before = plan.hostname.clone();
        }
        self.learn(plan.ip_addr.iter().chain(&plan.names).cloned());
    }

    // puts what was just learned first, keeping what was known after it
    fn learn(&mut self, places: impl IntoIterator<Item = String>) {
        let mut all: Vec<String> = vec![];
        for place in places.into_iter().chain(std::mem::take(&mut self.places)) {
            let place = place.trim().to_string();
            if usable_address(&place) && !all.iter().any(|p| p.eq_ignore_ascii_case(&place)) {
                all.push(place);
            }
        }
        self.places = all;
    }

    // everywhere to look but `ip`
    fn besides(&self, ip: &str) -> Vec<String> {
        self.places
            .iter()
            .filter(|p| !p.eq_ignore_ascii_case(ip))
            .cloned()
            .collect()
    }
}

pub async fn apply_with_retry(
    app: AppHandle,
    ip: String,
    profile: Profile,
    folder: Option<settings::Folder>,
) {
    let seen = {
        let state = app.state::<AppState>();
        let printer = state.printers.lock().unwrap().get(&ip).cloned();
        let recent = state.recent.lock().unwrap().clone();
        Mutex::new(Whereabouts::new(printer.as_ref(), &recent, &profile))
    };
    // where the printer is now; a restart can move it, e.g. firmware that renames it
    let at = Mutex::new(ip);
    let report = |s: Status| set_status(&app, &at.lock().unwrap(), s);
    let follow = |to: &str| moved(&app, &at.lock().unwrap(), to);
    let done = retry(&at, &profile, folder.as_ref(), &seen, &report, &follow).await;
    let ip = at.lock().unwrap().clone();
    let (serial, before, ip_addr) = {
        let seen = seen.lock().unwrap();
        (
            seen.serial.clone(),
            seen.hostname_before.clone(),
            seen.ip_addr.clone(),
        )
    };
    // what the printer goes by now, wherever the apply left it, so its row shows that
    let now = printer::identify(&ip)
        .await
        .ok()
        .filter(|id| serial.is_empty() || id.serial.as_deref() == Some(serial.as_str()));
    let ip = match &now {
        Some(id) => {
            let to = relist(&ip, before.as_deref(), id).await;
            if to != ip {
                moved(&app, &ip, &to);
            }
            to
        }
        None => ip,
    };
    update(&app, &ip, |p| {
        match now {
            Some(id) => {
                p.hostname = id.hostname.or(p.hostname.take());
                p.ip_addr = id.ip_addr.or(p.ip_addr.take());
                p.firmware = id.firmware.or(p.firmware.take());
            }
            // it didn't answer, but where it last did is worth keeping for the next apply
            None => p.ip_addr = ip_addr.or(p.ip_addr.take()),
        }
        p.status = done;
    });
}

// the address to list a printer under after applying. the one it's listed under stays while it
// still leads to the printer, unless it's the hostname the printer just gave up, which stops
// working once DNS catches up. otherwise its new hostname, if DNS points that at it, or its IP
async fn relist(ip: &str, before: Option<&str>, now: &printer::Identity) -> String {
    let Some(addr) = now.ip_addr.as_deref() else {
        return ip.to_string();
    };
    let renamed = before.is_some_and(|b| {
        !now.hostname
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(b))
    });
    let gave_up = renamed && before.is_some_and(|b| b.eq_ignore_ascii_case(ip));
    if !gave_up && leads_to(ip, addr).await {
        return ip.to_string();
    }
    match &now.hostname {
        Some(name) if leads_to(name, addr).await => name.clone(),
        _ => addr.to_string(),
    }
}

// whether `name` leads to `addr` as Windows looks it up: it's that IP, or DNS or WINS points it
// there
async fn leads_to(name: &str, addr: &str) -> bool {
    name == addr
        || printer::lookup(name)
            .await
            .iter()
            .any(|ip| ip.to_string() == addr)
}

// applies until it works or can't, backing off while the printer can't be reached. `at` is
// where the printer is; when it moves, `follow` is told before `at` changes
async fn retry(
    at: &Mutex<String>,
    profile: &Profile,
    folder: Option<&settings::Folder>,
    seen: &Mutex<Whereabouts>,
    report: &(dyn Fn(Status) + Sync),
    follow: &(dyn Fn(&str) + Sync),
) -> Status {
    let follow = |to: &str| {
        follow(to);
        *at.lock().unwrap() = to.to_string();
    };
    // without a known firmware version every attempt would install it again, restarting the
    // printer each time, so it's only installed once
    let flashed = AtomicBool::new(false);
    let mut attempt = 0u32;
    let mut delay = 5u64;
    loop {
        attempt += 1;
        report(Status::Applying("Connecting".into()));
        let ip = at.lock().unwrap().clone();
        match apply_once(&ip, profile, folder, &flashed, seen, report, &follow).await {
            Ok(checked) => return Status::Applied(checked),
            Err(ApplyError::Mismatch(m)) => return Status::Failed(m),
            Err(ApplyError::Unverified(m)) => return Status::Unverified(m),
            Err(ApplyError::Unreachable(reason)) => {
                // a profile can rename it and a restart move it to its other network, so before
                // waiting it's looked for wherever else it might answer
                if let Some(to) = look_elsewhere(&ip, seen, report).await {
                    follow(&to);
                    delay = 5;
                    continue;
                }
                report(Status::Offline {
                    attempt,
                    retry_in_secs: delay,
                    reason,
                });
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(20); // back off, cap at 20 s
            }
        }
    }
}

// where else the printer answers, if it has stopped answering at `ip`
async fn look_elsewhere(
    ip: &str,
    seen: &Mutex<Whereabouts>,
    report: &(dyn Fn(Status) + Sync),
) -> Option<String> {
    let (serial, places) = {
        let seen = seen.lock().unwrap();
        (seen.serial.clone(), seen.besides(ip))
    };
    if serial.is_empty() || places.is_empty() {
        return None;
    }
    report(Status::Applying("Looking for the printer".into()));
    // its own address first, so a printer that answers there again stays listed under it
    let addresses = std::iter::once(ip).chain(places.iter().map(String::as_str));
    let found = answers_at(addresses, &serial).await?;
    (!found.eq_ignore_ascii_case(ip)).then_some(found)
}

// the first of `addresses` where the printer answers with its serial, as what to reach it by
async fn answers_at<'a>(addresses: impl Iterator<Item = &'a str>, serial: &str) -> Option<String> {
    for place in addresses {
        for target in targets(place).await {
            if let Ok(got) = printer::query(&target, &["device.unique_id"]).await
                && got["device.unique_id"].as_str().map(str::trim) == Some(serial)
            {
                return Some(reach_by(place, &target).await);
            }
        }
    }
    None
}

// where to try a place: an IP as it is, a name at whatever IPs DNS has for it now
async fn targets(place: &str) -> Vec<String> {
    if place.parse::<IpAddr>().is_ok() {
        return vec![place.to_string()];
    }
    let found = printer::lookup_fresh(place).await;
    found.iter().map(IpAddr::to_string).collect()
}

// what to reach a printer by once it has answered at `ip`, found as `place`: the name while
// Windows' own lookups lead there too, since the app connects by it from then on, else the IP
async fn reach_by(place: &str, ip: &str) -> String {
    match leads_to(place, ip).await {
        true => place.to_string(),
        false => ip.to_string(),
    }
}

// what a profile sends to one printer, which depends on the printer and its firmware
struct Plan {
    serial: String,
    firmware: String,             // appl.name
    skipped: Vec<setup::Setting>, // settings the printer doesn't have
    extra: Vec<String>,
    label: Option<String>,    // the test label
    names: Vec<String>,       // hostnames it may answer to after a restart
    ip_addr: Option<String>,  // its IP on the network it's using
    hostname: Option<String>, // ip.hostname on Link-OS 7.6 and later, else the friendly name
    needs_restart: bool,      // it changes settings that only take effect at a restart
}

// settings that only take effect once the printer restarts
const ON_RESTART: [&str; 7] = [
    "ip.hostname",
    "device.friendly_name", // the hostname before Link-OS 7.6
    "internal_wired.ip.dhcp.option12",
    "wlan.ip.dhcp.option12",
    "internal_wired.ip.protocol",
    "wlan.ip.protocol",
    "ip.primary_network",
];

// whether the profile changes a setting that only takes effect at a restart, going by what the
// printer has now, so applying it again doesn't restart a printer for nothing
fn needs_restart(kept: &[setup::Setting], has: &serde_json::Value) -> bool {
    kept.iter().any(|s| {
        ON_RESTART.contains(&s.key.as_str())
            && !has[s.key.as_str()]
                .as_str()
                .is_some_and(|v| same_value(v, &s.value))
    })
}

async fn read_plan(ip: &str, p: &Profile) -> Result<Plan, ApplyError> {
    let unreachable = |e: anyhow::Error| ApplyError::Unreachable(e.to_string());
    let got = printer::query(
        ip,
        &[
            "head.resolution.in_dpi",
            "device.product_name",
            "device.unique_id",
            "device.friendly_name",
            "ip.hostname",
            "print.tone_zpl",
            "appl.name",
            "interface.network.active.ip_addr",
        ],
    )
    .await
    .map_err(unreachable)?;
    let get = |k: &str| got[k].as_str().unwrap_or("").trim().to_string();
    let dpi = get("head.resolution.in_dpi")
        .parse()
        .map_err(|_| ApplyError::Mismatch("printer didn't report its dpi".into()))?;
    let max_dots = match &p.media {
        Some(_) => printer::print_width_max(ip).await.map_err(unreachable)?,
        None => 0,
    };
    let target = setup::Target {
        serial: get("device.unique_id"),
        dpi,
        max_dots,
        // settings a printer doesn't have come back as null
        ip_hostname: got["ip.hostname"].is_string(),
        tone_zpl: got["print.tone_zpl"].is_string(),
    };
    // printers leave out settings they don't have, e.g. a mobile printer has no wired port, so
    // those are skipped rather than failing the read-back
    let wanted = setup::settings(p, Some(&target)).map_err(ApplyError::Mismatch)?;
    let keys: Vec<&str> = wanted.iter().map(|s| s.key.as_str()).collect();
    let has = match keys.is_empty() {
        true => serde_json::Value::Null,
        false => printer::query(ip, &keys).await.map_err(unreachable)?,
    };
    let (kept, skipped): (Vec<setup::Setting>, Vec<setup::Setting>) =
        wanted.into_iter().partition(|s| {
            has[s.key.as_str()]
                .as_str()
                .is_some_and(|v| v.trim() != "?")
        });
    let extra = setup::commands(&kept, &p.commands);
    // the test label, printed if there's media once everything is set
    let label = match &p.media {
        Some(m) => {
            let width = setup::width(m, dpi, max_dots).map_err(ApplyError::Mismatch)?;
            let lines = [
                p.name.clone(),
                format!("{} {}", get("device.product_name"), target.serial),
                format!("{width} dots at {dpi} dpi"),
                "zebra_config test print".into(),
            ];
            Some(setup::test_label(&lines, dpi))
        }
        None => None,
    };

    let names = restart_names(
        &[get("ip.hostname"), get("device.friendly_name")],
        &get("device.product_name"),
        &target.serial,
        &kept,
    );
    let hostname = [get("ip.hostname"), get("device.friendly_name")]
        .into_iter()
        .find(|h| !h.is_empty());
    Ok(Plan {
        firmware: got["appl.name"].as_str().unwrap_or("").to_string(),
        serial: target.serial,
        needs_restart: needs_restart(&kept, &has),
        skipped,
        extra,
        label,
        names,
        ip_addr: Some(get("interface.network.active.ip_addr")).filter(|a| usable_address(a)),
        hostname,
    })
}

// the hostnames a printer may answer to after a restart: the ones it has, {model}-{serial} as
// Link-OS 7.6 firmware names it, its serial, which older firmware uses until it's named, and
// the one the profile sets, which takes effect then
fn restart_names(
    current: &[String],
    product: &str,
    serial: &str,
    kept: &[setup::Setting],
) -> Vec<String> {
    let default_name = match product.is_empty() || serial.is_empty() {
        true => String::new(),
        false => format!("{product}-{serial}"),
    };
    let set = kept
        .iter()
        .filter(|s| s.key == "ip.hostname" || s.key == "device.friendly_name")
        .map(|s| s.value.clone());
    let mut names: Vec<String> = vec![];
    for name in current
        .iter()
        .cloned()
        .chain([default_name, serial.to_string()])
        .chain(set)
    {
        if !name.is_empty() && !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    names
}

// same order as a USB stick: firmware, files, then commands, with the profile's own settings
// among them (see with_profile). returns what was checked
async fn apply_once(
    ip: &str,
    p: &Profile,
    folder: Option<&settings::Folder>,
    flashed: &AtomicBool, // the firmware was installed by an earlier attempt
    seen: &Mutex<Whereabouts>,
    report: &(dyn Fn(Status) + Sync),
    follow: &(dyn Fn(&str) + Sync), // the printer came back at another address
) -> Result<String, ApplyError> {
    let unreachable = |e: anyhow::Error| ApplyError::Unreachable(e.to_string());
    let step = |s: &str| report(Status::Applying(s.into()));

    let usb = match folder.and_then(|f| f.path.as_deref()) {
        Some(path) => usb::load(Path::new(path))
            .map_err(|e| ApplyError::Mismatch(format!("can't read {path}: {e}")))?,
        None => usb::Folder::default(),
    };

    // worked out first, so a profile that doesn't fit the printer stops before anything is sent
    let mut ip = ip.to_string();
    let mut plan = read_plan(&ip, p).await?;
    seen.lock().unwrap().read(&plan);

    let mut checked = vec![];
    if let Some((file, firmware)) = usb
        .firmware
        .as_ref()
        .filter(|_| !flashed.load(Ordering::Relaxed))
    {
        // as set for the folder, or else going by the file's name, as a USB mirror does
        let set = folder.and_then(|f| f.firmware_version.as_deref());
        let want = set.unwrap_or(usb::firmware_version(file));
        if plan.firmware != want {
            step("Installing firmware");
            printer::send(&ip, firmware).await.map_err(unreachable)?;
            flashed.store(true, Ordering::Relaxed);
            step("Waiting for restart");
            // only a version that's set has to be reported, since a file can be named anything
            follow_restart(&mut ip, &plan.serial, seen, set, follow)
                .await
                .map_err(|looked| {
                    ApplyError::Mismatch(match set {
                        Some(v) => {
                            format!("printer did not report firmware {v} within 20 min; {looked}")
                        }
                        None => format!(
                            "printer did not restart within 20 min of the firmware download; {looked}"
                        ),
                    })
                })?;
            // new firmware can have settings the old one didn't, e.g. ip.hostname from 7.6
            plan = read_plan(&ip, p).await?;
            seen.lock().unwrap().read(&plan);
            checked.push(match plan.firmware == want {
                true => format!("firmware {want} installed"),
                // otherwise every apply installs it again
                false => format!(
                    "firmware installed, but it reports {0}, so set {0} as the folder's firmware version",
                    plan.firmware
                ),
            });
        }
    }

    if !usb.files.is_empty() {
        step("Sending files");
        for (name, data) in &usb.files {
            printer::send_file(&ip, name, data)
                .await
                .map_err(unreachable)?;
        }
    }

    let extra = if plan.extra.is_empty() {
        String::new()
    } else {
        plan.extra.join("\r\n") + "\r\n"
    };
    let folder_commands = usb.commands.iter().map(|(_, d)| d.as_slice()).collect();
    let commands = with_profile(folder_commands, extra.as_bytes());
    if !commands.is_empty() {
        step("Sending commands");
    }
    for (i, data) in commands.iter().enumerate() {
        if let Err(e) = printer::send(&ip, data).await {
            // an earlier command may have moved the printer to another network
            return Err(if i == 0 {
                unreachable(e)
            } else {
                ApplyError::Unverified(format!("printer stopped answering partway through: {e}"))
            });
        }
        if resets_printer(data) {
            step("Waiting for restart");
            follow_restart(&mut ip, &plan.serial, seen, None, follow)
                .await
                .map_err(|looked| {
                    ApplyError::Unverified(format!(
                        "printer did not come back after a command file reset it; {looked}"
                    ))
                })?;
        }
    }

    // hostname and network settings only take effect at a restart. when nothing restarted the
    // printer after the profile's, it's restarted and followed now, rather than moving later
    let restarted =
        resets_printer(extra.as_bytes()) || usb.commands.iter().any(|(_, d)| resets_printer(d));
    if plan.needs_restart && !restarted {
        step("Restarting for the network settings");
        printer::send(&ip, b"! U1 do \"device.reset\" \"\"\r\n")
            .await
            .map_err(|e| {
                ApplyError::Unverified(format!(
                    "sent everything, but couldn't restart the printer: {e}"
                ))
            })?;
        step("Waiting for restart");
        follow_restart(&mut ip, &plan.serial, seen, None, follow)
            .await
            .map_err(|looked| {
                ApplyError::Unverified(format!(
                    "restarted the printer for its network settings, and it didn't come back; {looked}"
                ))
            })?;
        checked.push("restarted for the network settings".to_string());
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
        let got = printer::query(&ip, &keys).await.map_err(stopped)?;
        let bad: Vec<&str> = expected
            .iter()
            .filter(|(k, v)| {
                !got.get(k)
                    .and_then(|x| x.as_str())
                    .is_some_and(|x| same_value(x, v))
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
        let on_printer = printer::files(&ip).await.map_err(stopped)?;
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

    if !plan.skipped.is_empty() {
        let keys: Vec<&str> = plan.skipped.iter().map(|s| s.key.as_str()).collect();
        checked.push(format!("skipped {} it doesn't have", keys.join(", ")));
    }
    if let Some(label) = &plan.label {
        checked.push(calibrate_and_print(&ip, label, &step).await?);
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
    if !h.errors.is_empty() {
        let e = h.errors.join(", ");
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
        if !h.errors.is_empty() || h.paused {
            recover(ip, step).await?;
            return Ok(
                "printer found no media when unpaused, so no calibration or test print".into(),
            );
        }
    }

    for (what, data) in [("Calibrating", "~JC"), ("Printing test label", label)] {
        step(what);
        printer::send(ip, data.as_bytes()).await.map_err(stopped)?;
        h = printer::wait_idle(ip, 60).await.map_err(stopped)?;
        if !h.errors.is_empty() || h.paused || h.status != "ready" {
            let problem = match h.errors.is_empty() {
                true => h.status,
                false => format!("an error ({})", h.errors.join(", ")),
            };
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
pub async fn recover(ip: &str, step: &(dyn Fn(&str) + Sync)) -> Result<(), ApplyError> {
    let stopped =
        |e: anyhow::Error| ApplyError::Unverified(format!("printer stopped answering: {e}"));
    step("Clearing printer error");
    printer::send(ip, b"~JA~JR").await.map_err(stopped)?;
    if wait_for_restart(ip, "", &[], None).await.is_none() {
        return Err(ApplyError::Unverified(
            "printer did not come back after resetting it to clear an error".into(),
        ));
    }
    printer::send(ip, b"! U1 do \"device.pause\" \"\"\r\n")
        .await
        .map_err(stopped)
}

// with a known firmware version, wait until the printer reports it; otherwise wait for it
// to drop off the network and come back. a restart can give it a new address, e.g. Link-OS 7.6
// renames it and can switch it to wireless, so with its serial known it's also looked for at
// `places`, and only taken where it answers with that serial. returns the address it answered
// at, or none after 20 min
async fn wait_for_restart(
    ip: &str,
    serial: &str,
    places: &[String],
    want: Option<&str>,
) -> Option<String> {
    let places = if serial.is_empty() { &[] } else { places };
    let start = Instant::now();
    let mut went_down = false;
    // where it answered as back on the last check. it has to answer twice in a row, since a
    // printer that has just started can stop answering again while it finishes, e.g. after new
    // firmware
    let mut last_back: Option<String> = None;
    while start.elapsed() < Duration::from_secs(20 * 60) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut answered = None;
        let addresses = std::iter::once(ip).chain(places.iter().map(String::as_str));
        'places: for (i, place) in addresses.enumerate() {
            let mut heard = false;
            for target in targets(place).await {
                // DHCP can hand the old address to another printer
                let got =
                    printer::query(&target, &["device.unique_id", "appl.name", "device.uptime"])
                        .await
                        .ok()
                        .filter(|got| {
                            serial.is_empty()
                                || got["device.unique_id"].as_str().map(str::trim) == Some(serial)
                        });
                let Some(got) = got else {
                    continue;
                };
                heard = true;
                // a printer busy writing firmware can stop answering for a while without
                // restarting, so it has restarted once it's been on for less time than this
                // wait. one without device.uptime has to be seen going down instead
                let restarted = match got["device.uptime"].as_str().and_then(parse_uptime) {
                    Some(up) => up < start.elapsed(),
                    None => went_down,
                };
                let back = match want {
                    Some(v) => got["appl.name"].as_str() == Some(v),
                    None => restarted,
                };
                if back {
                    answered = Some(reach_by(place, &target).await);
                    break 'places;
                }
            }
            went_down |= i == 0 && !heard;
        }
        match answered {
            Some(addr) if last_back.as_deref() == Some(addr.as_str()) => return Some(addr),
            other => last_back = other,
        }
    }
    None
}

// waits for the printer to restart, and follows it to wherever it answers. if it doesn't come
// back, says where it was looked for
async fn follow_restart(
    ip: &mut String,
    serial: &str,
    seen: &Mutex<Whereabouts>,
    want: Option<&str>,
    follow: &(dyn Fn(&str) + Sync),
) -> Result<(), String> {
    let places = seen.lock().unwrap().besides(ip);
    let Some(at) = wait_for_restart(ip, serial, &places, want).await else {
        return Err(looked_at(ip, &places));
    };
    if at != *ip {
        follow(&at);
        *ip = at;
    }
    Ok(())
}

// how long the printer has been on, from device.uptime, e.g. "00 days 00 hours 09 mins 34 secs"
fn parse_uptime(text: &str) -> Option<Duration> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || words.len() % 2 != 0 {
        return None;
    }
    let mut secs = 0u64;
    for pair in words.chunks(2) {
        let n: u64 = pair[0].parse().ok()?;
        secs += n * match pair[1].trim_end_matches('s') {
            "day" => 86400,
            "hour" => 3600,
            "min" => 60,
            "sec" => 1,
            _ => return None,
        };
    }
    Some(Duration::from_secs(secs))
}

fn looked_at(ip: &str, places: &[String]) -> String {
    let all: Vec<&str> = std::iter::once(ip)
        .chain(places.iter().map(String::as_str))
        .collect();
    format!("looked for it at {}", all.join(", "))
}

// the folder's command files in order, with the profile's own lines before the first one that
// resets the printer: then that restart puts the profile's hostname and network settings into
// effect while the app is still following the printer, rather than some later restart moving it
fn with_profile<'a>(mut commands: Vec<&'a [u8]>, profile: &'a [u8]) -> Vec<&'a [u8]> {
    if !profile.is_empty() {
        let before = commands
            .iter()
            .position(|d| resets_printer(d))
            .unwrap_or(commands.len());
        commands.insert(before, profile);
    }
    commands
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

// what the printer reads back can differ in case or format, e.g. "25.0" for "25"
pub fn same_value(got: &str, want: &str) -> bool {
    let got = got.trim();
    got.eq_ignore_ascii_case(want)
        || matches!((got.parse::<f64>(), want.parse::<f64>()), (Ok(a), Ok(b)) if a == b)
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
    fn read_back_values_compare_loosely() {
        assert!(same_value("25.0", "25"));
        assert!(same_value(" gap/notch ", "gap/notch"));
        assert!(same_value("ON", "on"));
        assert!(!same_value("24.5", "25"));
        assert!(!same_value("auto_detect", "gap/notch"));
    }

    #[test]
    fn looks_for_a_restarted_printer_by_every_name_it_may_have() {
        let set = |key: &str, value: &str| setup::Setting {
            label: "",
            key: key.into(),
            value: value.into(),
        };
        // a ZD611 on 7.5 named by its friendly name, getting a profile that renames it
        let names = restart_names(
            &["".into(), "zd611-dgn260506510".into()],
            "ZD611",
            "DGN260506510",
            &[
                set("ip.hostname", "CSCDGN260506510"),
                set("device.friendly_name", "CSCDGN260506510"),
                set("print.tone_zpl", "25.0"),
            ],
        );
        assert_eq!(
            names,
            ["zd611-dgn260506510", "DGN260506510", "CSCDGN260506510"]
        );
        // without a serial there's no default name to guess
        assert!(restart_names(&[], "ZD611", "", &[]).is_empty());
    }

    #[test]
    fn profile_settings_go_before_the_folders_reset() {
        let timezone: &[u8] = b"! U1 setvar \"rtc.timezone\" \"EST\"\r\n";
        let reset: &[u8] = b"{}{\"device.reset\" : \"\"}";
        let feedback: &[u8] = b"! U1 setvar \"usb.mirror.feedback.auto\" \"on\"\r\n";
        let profile: &[u8] = b"! U1 setvar \"ip.primary_network\" \"1\"\r\n";
        assert_eq!(
            with_profile(vec![timezone, reset, feedback], profile),
            [timezone, profile, reset, feedback]
        );
        // with nothing to reset the printer they go last, and a profile with none adds nothing
        assert_eq!(with_profile(vec![timezone], profile), [timezone, profile]);
        assert_eq!(with_profile(vec![timezone, reset], b""), [timezone, reset]);
    }

    fn zd611() -> Printer {
        Printer {
            ip: "ZD611-DGN260506510".into(),
            model: Some("ZD611".into()),
            serial: Some("DGN260506510".into()),
            firmware: None,
            profile_id: None,
            status: Status::Pending,
            hostname: Some("ZD611-DGN260506510".into()),
            ip_addr: Some("10.15.18.21".into()),
        }
    }

    fn recent(address: &str, serial: &str, hostname: &str) -> Recent {
        Recent {
            address: address.into(),
            model: None,
            serial: Some(serial.into()),
            hostname: Some(hostname.into()),
            ip_addr: Some(address.into()),
            last_used: 0,
        }
    }

    #[test]
    fn looks_for_a_printer_where_it_answered_and_by_the_names_it_may_have() {
        let renaming = Profile {
            id: "p".into(),
            name: "p".into(),
            models: vec![],
            hostname: Some("CSC{serial}".into()),
            darkness: None,
            network: None,
            media: None,
            commands: vec![],
        };
        // where the Printer page found it on wired, and another printer
        let recent = [
            recent("172.17.255.22", "DGN260506510", "CSCDGN260506510"),
            recent("172.17.255.9", "DFJ245200016", "CSCDFJ245200016"),
        ];
        let mut w = Whereabouts::new(Some(&zd611()), &recent, &renaming);
        assert_eq!(
            w.besides("zd611-dgn260506510"),
            [
                "10.15.18.21",
                "172.17.255.22",
                "CSCDGN260506510",
                "DGN260506510"
            ]
        );

        // a read puts where it is now first; an interface that's down doesn't count
        w.read(&Plan {
            serial: "DGN260506510".into(),
            firmware: String::new(),
            skipped: vec![],
            extra: vec![],
            label: None,
            names: vec!["ZD611-DGN260506510".into()],
            ip_addr: Some("10.15.18.30".into()),
            hostname: Some("ZD611-DGN260506510".into()),
            needs_restart: false,
        });
        w.learn(["0.0.0.0".to_string()]);
        assert_eq!(w.ip_addr.as_deref(), Some("10.15.18.30"));
        assert_eq!(
            w.besides("ZD611-DGN260506510")[..2],
            ["10.15.18.30", "10.15.18.21"]
        );

        // a printer added without its serial can't be told apart from another, so isn't looked for
        let mut nameless = zd611();
        nameless.serial = None;
        assert!(
            Whereabouts::new(Some(&nameless), &recent, &renaming)
                .besides("x")
                .is_empty()
        );
    }

    #[test]
    fn restarts_only_for_network_settings_the_printer_doesnt_have() {
        let set = |key: &str, value: &str| setup::Setting {
            label: "",
            key: key.into(),
            value: value.into(),
        };
        let has = serde_json::json!({
            "print.tone_zpl": "20.0",
            "device.friendly_name": "CSCDGN260506510",
            "ip.primary_network": "2",
        });
        // darkness takes effect straight away, and the printer already has the name
        assert!(!needs_restart(
            &[
                set("print.tone_zpl", "25.0"),
                set("device.friendly_name", "CSCDGN260506510")
            ],
            &has
        ));
        // the preferred network only changes at a restart
        assert!(needs_restart(&[set("ip.primary_network", "1")], &has));
    }

    #[tokio::test]
    async fn tries_an_ip_as_it_is_and_a_name_at_what_dns_has() {
        assert_eq!(targets("192.0.2.10").await, ["192.0.2.10"]);
        assert!(targets("nothing.invalid").await.is_empty());
        assert!(
            targets("localhost")
                .await
                .iter()
                .any(|ip| ip.starts_with("127."))
        );
    }

    // .invalid names never resolve
    #[tokio::test]
    async fn relists_a_printer_that_gave_up_its_name() {
        let now = printer::Identity {
            hostname: Some("cscdgn260506510.invalid".into()),
            ip_addr: Some("192.0.2.10".into()),
            ..Default::default()
        };
        let old = "zd611-dgn260506510.invalid";
        // listed under the name the profile replaced, and DNS doesn't have the new one: its IP
        assert_eq!(relist(old, Some(old), &now).await, "192.0.2.10");
        // listed under its IP, it stays
        assert_eq!(relist("192.0.2.10", Some(old), &now).await, "192.0.2.10");
        // an IP it doesn't have any more is replaced by the one it has
        assert_eq!(relist("192.0.2.99", None, &now).await, "192.0.2.10");
        // without an IP to go by, nothing moves
        let unknown = printer::Identity::default();
        assert_eq!(relist("192.0.2.99", None, &unknown).await, "192.0.2.99");
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

    #[test]
    fn reads_uptime() {
        assert_eq!(
            parse_uptime("00 days 00 hours 09 mins 34 secs"),
            Some(Duration::from_secs(9 * 60 + 34))
        );
        assert_eq!(
            parse_uptime("2 days 1 hour 0 mins 1 sec"),
            Some(Duration::from_secs(2 * 86400 + 3600 + 1))
        );
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_uptime("?"), None);
        assert_eq!(parse_uptime("09 mins 34"), None);
    }

    // talks to a real printer, so it only runs when asked, and retries as the app does. both env
    // vars after the IP are optional:
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
                hostname: None,
                darkness: None,
                network: None,
                media: None,
                commands: vec![],
            });
        let start = Instant::now();
        let log = |s: &str| println!("{:>5}s {s}", start.elapsed().as_secs());
        log(&format!("before: {:?}", printer::identify(&ip).await.ok()));
        let at = Mutex::new(ip);
        let report = |s: Status| log(&serde_json::to_string(&s).unwrap());
        let follow = |to: &str| log(&format!("moved to {to}"));
        let seen = Mutex::new(Whereabouts::default());
        let done = retry(&at, &p, folder.as_ref(), &seen, &report, &follow).await;
        report(done.clone());
        let ip = at.lock().unwrap().clone();
        let now = printer::identify(&ip).await.ok();
        log(&format!("after, at {ip}: {now:?}"));
        if let Some(now) = &now {
            let before = seen.lock().unwrap().hostname_before.clone();
            log(&format!(
                "the app would list it as {}",
                relist(&ip, before.as_deref(), now).await
            ));
        }
        assert!(matches!(done, Status::Applied(_)));
    }
}
