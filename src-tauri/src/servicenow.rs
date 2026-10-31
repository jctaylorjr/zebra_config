// asks ServiceNow about printers, through Edge (browser.rs) signed in as the person using the
// app, the way dwo_deploy does: the instance's own pages carry the session's token in g_ck, and
// a fetch from one of them with it in an X-UserToken header reads the Table API as that person,
// so the app needs no password or account of its own
use crate::browser::{self, Browser, Page, js_str};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

pub const INSTANCE: &str = "https://partnershealthcare.service-now.com";

// the classic page's frame. the Next Experience shell keeps it inside shadow DOM, where
// querySelector alone doesn't look
const MAIN_FRAME_JS: &str = r#"(function find(root) {
    const f = root.querySelector('iframe#gsft_main, iframe[name="gsft_main"]');
    if (f) return f;
    for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) { const inner = find(el.shadowRoot); if (inner) return inner; }
    }
    return null;
})(document)"#;

// the session's token, in the page or its gsft_main frame
fn token_js() -> String {
    format!("(window.g_ck || ({MAIN_FRAME_JS} || {{}}).contentWindow?.g_ck || '')")
}

// the signed-in tab the app asks ServiceNow through, opened the first time it's needed
#[derive(Default)]
pub struct ServiceNow {
    browser: browser::Shared,
    page: tokio::sync::Mutex<Option<Page>>,
}

#[derive(Deserialize)]
struct Fetched {
    status: u16,
    text: String,
}

// a printer as ServiceNow has it: its record is named by its control number. a retired one's
// name has " [Retired]" after the number, as its serial does
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub name: String,
    pub serial: String,
    pub model: String, // like "Zebra ZD611D-HC Desktop Direct Thermal Printer - Monochrome - ..."
    pub location: String,
    pub room: String,
}

impl Record {
    // where it is, for saying which printer was found
    pub fn place(&self) -> String {
        match (self.location.trim(), self.room.trim()) {
            ("", "") => String::new(),
            (l, "") => l.to_string(),
            ("", r) => format!("room {r}"),
            (l, r) => format!("{l}, room {r}"),
        }
    }
}

fn records(v: &Value) -> Vec<Record> {
    let text = |r: &Value, k: &str| r[k].as_str().unwrap_or("").trim().to_string();
    v.as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| Record {
                    name: text(r, "name"),
                    serial: text(r, "serial_number"),
                    model: text(r, "model_id"),
                    location: text(r, "location"),
                    room: text(r, "u_room__cube"),
                })
                .collect()
        })
        .unwrap_or_default()
}

impl ServiceNow {
    // the printers ServiceNow has under control number `n`, which is only digits; not retired
    // ones, as their names have more after the number
    pub async fn printers(
        &self,
        profile: &Path,
        n: &str,
        report: &(dyn Fn(&str) + Sync),
    ) -> Result<Vec<Record>> {
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            bail!("{n:?} isn't a control number");
        }
        let query = format!("name={n}");
        let path = format!(
            "/api/now/table/cmdb_ci_printer?sysparm_query={}&sysparm_fields=name,serial_number,model_id,location,u_room__cube&sysparm_display_value=true&sysparm_exclude_reference_link=true&sysparm_limit=20",
            enc(&query)
        );
        Ok(records(&self.get(profile, &path, report).await?))
    }

    // a Table API read, like "/api/now/table/alm_hardware?sysparm_query=..."; returns its result.
    // `profile` is the browser's profile folder, and `report` hears what it's waiting for
    pub async fn get(
        &self,
        profile: &Path,
        path: &str,
        report: &(dyn Fn(&str) + Sync),
    ) -> Result<Value> {
        for attempt in 0..2 {
            let page = self.page(profile, report).await?;
            let js = format!(
                r#"(async () => {{
                    const r = await fetch({path}, {{
                        credentials: 'same-origin',
                        headers: {{ 'Accept': 'application/json', 'X-UserToken': {token} }}
                    }});
                    return {{ status: r.status, text: await r.text() }};
                }})()"#,
                path = js_str(path),
                token = token_js(),
            );
            let r: Fetched = match page.eval(&js).await {
                Ok(r) => r,
                // the tab went on to another page as it was asked, as the end of a sign-in can
                // send it, or closed; asked again in it, or in a new one
                Err(_) if attempt == 0 => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if !page.alive() {
                        self.forget(&page).await;
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            let json = serde_json::from_str::<Value>(&r.text).ok();
            match (r.status, json) {
                (200..=299, Some(v)) => return Ok(v.get("result").cloned().unwrap_or(v)),
                // signed out: a sign-in page instead of JSON, or 401. signs in again and asks once more
                (200..=299, None) | (401, _) if attempt == 0 => {
                    self.forget(&page).await;
                    continue;
                }
                (status, json) => bail!("{}", describe(status, json.as_ref(), &r.text)),
            }
        }
        bail!("ServiceNow kept saying the session isn't signed in")
    }

    // forgets `page` if it's still the tab in use, so calls that failed together on a tab that
    // signed out don't close the new one another opened
    async fn forget(&self, page: &Page) {
        let mut slot = self.page.lock().await;
        if slot.as_ref().is_some_and(|p| p.same(page))
            && let Some(p) = slot.take()
        {
            p.close().await;
        }
    }

    // the signed-in tab, signing in first if needed
    async fn page(&self, profile: &Path, report: &(dyn Fn(&str) + Sync)) -> Result<Page> {
        let mut slot = self.page.lock().await;
        if let Some(p) = slot.as_ref().filter(|p| p.alive()) {
            return Ok(p.clone());
        }
        report("Opening ServiceNow");
        let page = self.browser.get(profile).await?.new_page().await?;
        // an empty list loads quickly and has the session's token
        let start = format!("{INSTANCE}/task_list.do?sysparm_query=sys_id%3D-1");
        // one that can't be opened at all, like off the VPN, fails now, as waiting for a sign-in
        // won't help; one slow to load, or sent on to sign in, is for signed_in to judge
        match page.goto(&start, Duration::from_secs(45)).await {
            Err(e) if browser::unreachable(&e) => {
                page.close().await;
                bail!("can't reach ServiceNow ({e:#}); is this PC on the network or the VPN?");
            }
            _ => {}
        }
        if let Err(e) = signed_in(&page, report).await {
            page.close().await;
            return Err(e);
        }
        *slot = Some(page.clone());
        Ok(page)
    }
}

// waits until the tab is signed in to the instance. single sign-on usually manages by itself,
// passing through Microsoft's sign-in page on the way, so the browser is only shown when the tab
// stays on a sign-in page for a few seconds or 20 seconds pass; then it waits up to 15 minutes
// for someone to sign in there
async fn signed_in(page: &Page, report: &(dyn Fn(&str) + Sync)) -> Result<()> {
    let host = INSTANCE.split("://").nth(1).unwrap_or(INSTANCE);
    let ready_js = format!(
        "location.host === {} && !location.pathname.startsWith('/login') && !!{} && location.href",
        js_str(host),
        token_js()
    );
    // the same page twice, half a second apart, as the end of a sign-in passes through a page
    // with a token that goes straight on to the one asked for
    let ready = || async {
        let first = page.eval::<Value>(&ready_js).await.ok();
        let Some(Value::String(href)) = first else {
            return false;
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        page.eval::<Value>(&ready_js).await.ok() == Some(Value::String(href))
    };
    let start = Instant::now();
    let mut on_sign_in: Option<Instant> = None;
    loop {
        if ready().await {
            return Ok(());
        }
        if !page.alive() {
            bail!("the ServiceNow tab was closed");
        }
        if !is_sign_in_page(&page.url().await) {
            on_sign_in = None;
        } else if on_sign_in.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(5) {
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    report("Sign in to ServiceNow in the browser window");
    // the window hides again however the wait ends, being called off too
    struct Signing(Arc<Browser>);
    impl Drop for Signing {
        fn drop(&mut self) {
            self.0.end_sign_in();
        }
    }
    let _signing = Signing(page.browser.clone());
    page.browser.begin_sign_in(page).await;
    let deadline = Instant::now() + Duration::from_secs(900);
    while Instant::now() < deadline && page.alive() {
        if ready().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if page.alive() {
        bail!("not signed in to ServiceNow after 15 minutes");
    }
    bail!("the sign-in window was closed before signing in to ServiceNow")
}

// pages where someone has to type something: Microsoft's sign-in, ADFS, ServiceNow's own login
fn is_sign_in_page(url: &str) -> bool {
    let url = url.to_lowercase();
    let host = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?', '#']).next())
        .unwrap_or("");
    [
        "login.microsoftonline.com",
        "login.microsoft.com",
        "login.windows.net",
        "login.live.com",
        "device.login.microsoftonline.com",
        "autologon.microsoftazuread-sso.com",
    ]
    .contains(&host)
        || host.contains("okta")
        || host.contains("pingone")
        || host.contains("pingfed")
        || url.contains("/adfs/")
        || url.contains("/login.do")
        || url.contains("/saml")
}

// why ServiceNow refused, in its own words when it gave some
fn describe(status: u16, json: Option<&Value>, text: &str) -> String {
    let say = |key: &str| {
        json.and_then(|j| j.pointer(key))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    let (message, detail) = (say("/error/message"), say("/error/detail"));
    let what = match status {
        403 => "ServiceNow didn't allow it",
        404 => "ServiceNow couldn't find it",
        _ => "ServiceNow refused",
    };
    match (message.is_empty(), detail.is_empty()) {
        (false, false) => format!("{what}: {message} ({detail})"),
        (false, true) => format!("{what}: {message}"),
        _ => {
            let excerpt: String = text.chars().take(160).collect();
            format!("{what} (HTTP {status}) {excerpt}")
        }
    }
}

// `s` for a URL's query string
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_sign_in_pages() {
        assert!(is_sign_in_page(
            "https://login.microsoftonline.com/common/oauth2/authorize?x=1"
        ));
        assert!(is_sign_in_page(
            "https://sts.partners.org/adfs/ls/?SAMLRequest=abc"
        ));
        assert!(is_sign_in_page(
            "https://partnershealthcare.service-now.com/login.do"
        ));
        assert!(!is_sign_in_page(
            "https://partnershealthcare.service-now.com/task_list.do?sysparm_query=x"
        ));
        assert!(!is_sign_in_page("about:blank"));
    }

    #[test]
    fn encodes_query_text() {
        assert_eq!(
            enc("serial_number=AB12^ORname=CSC-1"),
            "serial_number%3DAB12%5EORname%3DCSC-1"
        );
    }

    #[test]
    fn reads_printer_records() {
        let v = serde_json::json!([
            {
                "name": "722858",
                "serial_number": "DGN260506471",
                "model_id": "Zebra ZD611D-HC Desktop Direct Thermal Printer",
                "location": "MGH/CRP-Charles River Plaza East/Floor 08",
                "u_room__cube": "820"
            },
            { "name": "csc722858", "serial_number": "", "model_id": "Zebra Printer", "location": "", "u_room__cube": "" }
        ]);
        let found = records(&v);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].serial, "DGN260506471");
        assert_eq!(
            found[0].place(),
            "MGH/CRP-Charles River Plaza East/Floor 08, room 820"
        );
        assert_eq!(found[1].place(), "");
        assert!(records(&serde_json::json!({ "error": "x" })).is_empty());
    }

    #[test]
    fn says_why_it_was_refused() {
        let j = serde_json::json!({ "error": { "message": "No Record found", "detail": "" } });
        assert_eq!(
            describe(404, Some(&j), ""),
            "ServiceNow couldn't find it: No Record found"
        );
        assert_eq!(
            describe(500, None, "oops"),
            "ServiceNow refused (HTTP 500) oops"
        );
    }

    // reads ServiceNow as the person signed in to the app's browser, which shows itself for a
    // sign-in if one is needed: ZEBRA_SN_GET holds Table API paths, a line each
    #[tokio::test]
    #[ignore]
    async fn ask_servicenow() {
        let paths = std::env::var("ZEBRA_SN_GET").expect("set ZEBRA_SN_GET");
        let profile = std::env::var("ZEBRA_SN_PROFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                Path::new(&std::env::var("APPDATA").unwrap())
                    .join("com.jt1280.zebra-configure")
                    .join("browser-edge")
            });
        let sn = ServiceNow::default();
        let began = Instant::now();
        let report = |s: &str| println!("... {:.1}s {s}", began.elapsed().as_secs_f32());
        for path in paths.lines().map(str::trim).filter(|p| !p.is_empty()) {
            report(&format!("GET {path}"));
            match sn.get(&profile, path, &report).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => println!("ERROR {e:#}"),
            }
        }
    }
}
