// a small Chrome DevTools Protocol client for the browser the app drives to reach ServiceNow:
// Microsoft Edge, started with a profile of its own so its sign-in is kept between runs, the way
// dwo_deploy reaches it. its windows stay hidden unless someone has to sign in
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

// where Edge is installed, if it is
pub fn find_edge() -> Option<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join(r"Microsoft\Edge\Application\msedge.exe"))
        .find(|p| p.is_file())
}

// switches every launch gets. the throttling ones keep pages working at full speed while their
// window is hidden
const SWITCHES: &[&str] = &[
    "--remote-debugging-port=0",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--disable-features=CalculateNativeWinOcclusion,IntensiveWakeUpThrottling",
    // no "Restore pages?" prompt: the browser ends with the app, which it takes for a crash
    "--hide-crash-restore-bubble",
    // opens out of sight, so it doesn't flash up before the app hides it
    "--window-position=-32000,-32000",
    "--window-size=1100,800",
];

#[derive(Clone, Debug)]
struct Event {
    session: Option<String>,
    method: String,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Browser {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    events: broadcast::Sender<Event>,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
    detached: Arc<Mutex<HashSet<String>>>, // sessions whose tab was closed or crashed
    windows: Arc<Windows>,
    child: Mutex<Option<std::process::Child>>,
}

// the browser's windows show only while someone is signing in
#[derive(Default)]
struct Windows {
    pid: AtomicU64,          // the browser process, whose windows these are; 0 if unknown
    signing_in: AtomicUsize, // sign-ins waiting for a person
    hidden: Mutex<Vec<isize>>, // windows the app hid, to show again
}

impl Browser {
    // connects to the browser already running with this profile, or else starts one. one that's
    // running was started by another copy of the app, as each one's ends with it, and stays that
    // copy's: taken over, it would end when this copy does, under the other one
    pub async fn launch(exe: &Path, profile: &Path) -> Result<Arc<Browser>> {
        let port_file = profile.join("DevToolsActivePort");
        if let Some(url) = read_port_file(&port_file)
            && let Ok(b) = connect(&url).await
        {
            return Ok(b);
        }
        // a browser closed just now can take a moment to let go of the profile, and a new one
        // started before then hands over to it and quits; it's started again
        for attempt in 0..3 {
            let _ = fs::remove_file(&port_file);
            fs::create_dir_all(profile)
                .with_context(|| format!("can't create {}", profile.display()))?;
            let mut child = std::process::Command::new(exe)
                .arg(format!("--user-data-dir={}", profile.display()))
                .args(SWITCHES)
                .arg("about:blank")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .with_context(|| format!("couldn't start {}", exe.display()))?;
            // it ends with the app, even if the app crashes: a hidden, signed-in browser mustn't
            // be left running
            job::hold_child(&child);
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(url) = read_port_file(&port_file)
                    && let Ok(b) = connect(&url).await
                {
                    let _ = b.windows.pid.compare_exchange(
                        0,
                        u64::from(child.id()),
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                    *b.child.lock().unwrap() = Some(child);
                    return Ok(b);
                }
                if let Ok(Some(status)) = child.try_wait() {
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        break;
                    }
                    bail!("Edge closed as soon as it opened ({status})");
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    bail!("Edge didn't open its DevTools port, which a policy can block");
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
        unreachable!("the last attempt returns or fails")
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(session, method, params, Duration::from_secs(60))
            .await
    }

    async fn call_timeout(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        if self.is_closed() {
            bail!("the browser closed");
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        if self.out.send(msg.to_string()).is_err() {
            self.pending.lock().unwrap().remove(&id);
            bail!("the browser closed");
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!("{method}: {e}")),
            Ok(Err(_)) => Err(anyhow!("the browser closed")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("{method} timed out"))
            }
        }
    }

    // a new tab, in the background
    pub async fn new_page(self: &Arc<Self>) -> Result<Page> {
        let r = self
            .call(
                None,
                "Target.createTarget",
                json!({ "url": "about:blank", "background": true }),
            )
            .await?;
        let target = r["targetId"].as_str().context("no tab id")?.to_string();
        let r = self
            .call(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .await?;
        let session = r["sessionId"]
            .as_str()
            .context("no session id")?
            .to_string();
        let page = Page {
            browser: self.clone(),
            target,
            session,
        };
        page.call("Page.enable", json!({})).await?;
        let _ = page.call("Inspector.enable", json!({})).await;
        // a hidden window isn't focused, and some pages wait for focus before they do anything
        let _ = page
            .call(
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": true }),
            )
            .await;
        Ok(page)
    }

    // shows the browser at `page` for someone to sign in there; end_sign_in hides it again
    pub async fn begin_sign_in(&self, page: &Page) {
        self.windows.signing_in.fetch_add(1, Ordering::SeqCst);
        show_windows(&self.windows);
        // it opens off the screen, so its window is brought where it can be seen
        if let Ok(w) = self
            .call(
                None,
                "Browser.getWindowForTarget",
                json!({ "targetId": page.target }),
            )
            .await
        {
            let id = w["windowId"].clone();
            let _ = self
                .call(
                    None,
                    "Browser.setWindowBounds",
                    json!({ "windowId": id, "bounds": { "windowState": "normal" } }),
                )
                .await;
            let _ = self
                .call(
                    None,
                    "Browser.setWindowBounds",
                    json!({ "windowId": id, "bounds": { "left": 120, "top": 80, "width": 1100, "height": 800 } }),
                )
                .await;
        }
        show_windows(&self.windows);
        allow_front(&self.windows);
        let _ = self
            .call(
                None,
                "Target.activateTarget",
                json!({ "targetId": page.target }),
            )
            .await;
    }

    pub fn end_sign_in(&self) {
        let count = &self.windows.signing_in;
        let mut n = count.load(Ordering::SeqCst);
        while n > 0 {
            match count.compare_exchange(n, n - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
        if count.load(Ordering::SeqCst) == 0 {
            hide_windows(&self.windows);
        }
    }
}

fn read_port_file(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

async fn connect(url: &str) -> Result<Arc<Browser>> {
    // a page's answer can be big, so there's room for one well over tungstenite's own limit
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(64 << 20))
        .max_frame_size(Some(64 << 20));
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async_with_config(url, Some(config), false),
    )
    .await
    .context("the browser didn't answer")??;
    let (mut sink, mut stream) = ws.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
    let pending: Pending = Default::default();
    let (events, _) = broadcast::channel(1024);
    let closed = Arc::new(AtomicBool::new(false));
    let detached: Arc<Mutex<HashSet<String>>> = Default::default();

    tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    {
        let (pending, events, closed, detached, out) = (
            pending.clone(),
            events.clone(),
            closed.clone(),
            detached.clone(),
            out.clone(),
        );
        tokio::spawn(async move {
            // ids for the dialogs this answers itself, apart from the calls' ids
            let mut dialog_id = u64::MAX / 2;
            while let Some(msg) = stream.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(match v.get("error") {
                            Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        });
                    }
                    continue;
                }
                let Some(method) = v.get("method").and_then(Value::as_str) else {
                    continue;
                };
                let session = v.get("sessionId").and_then(Value::as_str).map(String::from);
                match method {
                    "Target.detachedFromTarget" => {
                        if let Some(s) = v.pointer("/params/sessionId").and_then(Value::as_str) {
                            detached.lock().unwrap().insert(s.to_string());
                        }
                    }
                    "Inspector.targetCrashed" => {
                        if let Some(s) = &session {
                            detached.lock().unwrap().insert(s.clone());
                        }
                    }
                    // an alert would stop the page until someone answered it, in a window
                    // nobody can see
                    "Page.javascriptDialogOpening" => {
                        if let Some(s) = &session {
                            dialog_id += 1;
                            let _ = out.send(
                                json!({
                                    "id": dialog_id,
                                    "sessionId": s,
                                    "method": "Page.handleJavaScriptDialog",
                                    "params": { "accept": true }
                                })
                                .to_string(),
                            );
                        }
                    }
                    _ => {}
                }
                let _ = events.send(Event {
                    session,
                    method: method.to_string(),
                });
            }
            closed.store(true, Ordering::SeqCst);
            for (_, tx) in pending.lock().unwrap().drain() {
                let _ = tx.send(Err("the browser closed".into()));
            }
        });
    }

    let browser = Arc::new(Browser {
        out,
        pending,
        events,
        next_id: AtomicU64::new(1),
        closed,
        detached,
        windows: Default::default(),
        child: Mutex::new(None),
    });
    if let Ok(r) = browser
        .call(None, "SystemInfo.getProcessInfo", json!({}))
        .await
        && let Some(pid) = r["processInfo"]
            .as_array()
            .and_then(|list| list.iter().find(|p| p["type"] == "browser"))
            .and_then(|p| p["id"].as_u64())
    {
        browser.windows.pid.store(pid, Ordering::SeqCst);
    }
    // new windows, like a sign-in popup, show up on their own; they're hidden as they appear,
    // unless someone is signing in
    let weak = Arc::downgrade(&browser);
    tokio::spawn(async move {
        while let Some(b) = weak.upgrade() {
            if b.is_closed() {
                break;
            }
            if b.windows.signing_in.load(Ordering::SeqCst) == 0 {
                hide_windows(&b.windows);
            }
            drop(b);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });
    Ok(browser)
}

// one tab, driven through its own session
#[derive(Clone)]
pub struct Page {
    pub browser: Arc<Browser>,
    target: String,
    session: String,
}

impl Page {
    pub fn alive(&self) -> bool {
        !self.browser.is_closed()
            && !self
                .browser
                .detached
                .lock()
                .unwrap()
                .contains(&self.session)
    }

    pub fn same(&self, other: &Page) -> bool {
        self.session == other.session
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.browser.call(Some(&self.session), method, params).await
    }

    // runs a script in the page, waiting for its promise; what it returns comes back as JSON
    pub async fn eval<T: DeserializeOwned>(&self, js: &str) -> Result<T> {
        let r = self
            .browser
            .call_timeout(
                Some(&self.session),
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": true
                }),
                Duration::from_secs(60),
            )
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            let text = ex
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| ex["text"].as_str())
                .unwrap_or("the script failed");
            bail!("{}", text.lines().next().unwrap_or(text));
        }
        let v = r.pointer("/result/value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(v).context("the page answered with something unexpected")
    }

    pub async fn url(&self) -> String {
        self.eval::<String>("location.href")
            .await
            .unwrap_or_default()
    }

    // opens `url` and waits for it to load
    pub async fn goto(&self, url: &str, timeout: Duration) -> Result<()> {
        let mut events = self.browser.events.subscribe();
        let r = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(Unreachable(format!("couldn't open {url}: {e}")).into());
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, events.recv()).await {
                Err(_) => bail!("{url} took too long to load"),
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => bail!("the browser closed"),
                Ok(Ok(e)) => {
                    if e.session.as_deref() == Some(self.session.as_str())
                        && e.method == "Page.loadEventFired"
                    {
                        return Ok(());
                    }
                }
            }
        }
    }

    pub async fn close(&self) {
        let _ = self
            .browser
            .call(
                None,
                "Target.closeTarget",
                json!({ "targetId": self.target }),
            )
            .await;
    }
}

// a page that couldn't be opened at all, like a server that can't be reached off the VPN, as
// against one that was slow to load
#[derive(Debug)]
pub struct Unreachable(String);

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreachable {}

pub fn unreachable(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Unreachable>().is_some()
}

// a JavaScript string literal holding `s`
pub fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

// the browser the app is using, started the first time something needs it and again if it closed
#[derive(Default)]
pub struct Shared {
    current: tokio::sync::Mutex<Option<Arc<Browser>>>,
}

impl Shared {
    pub async fn get(&self, profile: &Path) -> Result<Arc<Browser>> {
        let mut current = self.current.lock().await;
        if let Some(b) = current.as_ref().filter(|b| !b.is_closed()) {
            return Ok(b.clone());
        }
        let exe = find_edge().context("Microsoft Edge isn't installed where the app looked")?;
        let b = Browser::launch(&exe, profile).await?;
        *current = Some(b.clone());
        Ok(b)
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindow,
            IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
        },
    };

    struct Search {
        pid: u32,
        found: Vec<isize>,
    }

    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: lparam is the Search that visible() passed, alive for the whole EnumWindows
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == search.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            search.found.push(hwnd as isize);
        }
        1
    }

    // the process's top-level windows that can be seen
    pub fn visible(pid: u32) -> Vec<isize> {
        let mut search = Search { pid, found: vec![] };
        unsafe { EnumWindows(Some(each), &mut search as *mut Search as LPARAM) };
        search.found
    }

    pub fn hide(hwnd: isize) {
        unsafe { ShowWindow(hwnd as HWND, SW_HIDE) };
    }

    pub fn show(hwnd: isize, front: bool) {
        let h = hwnd as HWND;
        unsafe {
            if IsWindow(h) == 0 {
                return;
            }
            ShowWindow(h, SW_SHOW);
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            if front {
                SetForegroundWindow(h);
            }
        }
    }

    // lets the browser bring its window in front of other programs', like the person's own Edge
    pub fn allow_front(pid: u32) {
        unsafe { AllowSetForegroundWindow(pid) };
    }
}

fn hide_windows(w: &Windows) {
    #[cfg(windows)]
    {
        let pid = w.pid.load(Ordering::SeqCst) as u32;
        if pid == 0 {
            return;
        }
        let mut hidden = w.hidden.lock().unwrap();
        for hwnd in win::visible(pid) {
            win::hide(hwnd);
            if !hidden.contains(&hwnd) {
                hidden.push(hwnd);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = w;
}

fn show_windows(w: &Windows) {
    #[cfg(windows)]
    {
        let hidden: Vec<isize> = w.hidden.lock().unwrap().drain(..).collect();
        let last = hidden.len().saturating_sub(1);
        for (i, hwnd) in hidden.into_iter().enumerate() {
            win::show(hwnd, i == last);
        }
    }
    #[cfg(not(windows))]
    let _ = w;
}

fn allow_front(w: &Windows) {
    #[cfg(windows)]
    {
        let pid = w.pid.load(Ordering::SeqCst) as u32;
        if pid != 0 {
            win::allow_front(pid);
        }
    }
    #[cfg(not(windows))]
    let _ = w;
}

// a Windows job object the app puts the browser into, set to end everything in it when the
// app's handle to it closes, as it does when the app ends, cleanly or not
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    struct Job(HANDLE);
    // SAFETY: a job handle can be used from any thread
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    // the app's one job, made the first time it's needed and never closed by the app itself, so
    // Windows closes it only when the app ends
    fn shared() -> HANDLE {
        static JOB: OnceLock<Job> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Job(std::ptr::null_mut());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                CloseHandle(job);
                return Job(std::ptr::null_mut());
            }
            Job(job)
        })
        .0
    }

    pub fn hold_child(child: &std::process::Child) {
        let (job, process) = (shared(), child.as_raw_handle() as HANDLE);
        if !job.is_null() && !process.is_null() {
            unsafe { AssignProcessToJobObject(job, process) };
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub fn hold_child(_child: &std::process::Child) {}
}
