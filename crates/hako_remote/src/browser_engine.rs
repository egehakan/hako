//! The Mac browser at work, on the connection's own thread and out of the app's way: it starts
//! Chrome for the first viewer and quits it when none has been left for a while, keeps each
//! viewer's tabs, passes pictures of the active tab to the relay as they come and acknowledges
//! them to Chrome only once the viewer has, and turns the viewer's requests and input into
//! DevTools commands.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_tungstenite::tungstenite::Message;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval, sleep};

use crate::browser::{
    AckParams, BrowserMethod, ClosedEvent, ClosedReason, ConsoleEvent, ConsoleLevel, CopyResult,
    DialogClosedEvent, DialogEvent, DialogKind, DialogReplyParams, DownloadEvent, EVENT_CLOSED,
    EVENT_CONSOLE, EVENT_DIALOG, EVENT_DIALOG_CLOSED, EVENT_DOWNLOAD, EVENT_FILE_CHOOSER,
    EVENT_PICKER, EVENT_STATE, FileChooserEvent, FileChooserReplyParams, FrameHeader, ImageFormat,
    InputParams, MAX_CONSOLE_CHARS, MAX_CONSOLE_MESSAGES, NavigateParams, OpenParams, OpenResult,
    PagePicker, PickerEvent, PickerReplyParams, PreviewAddresses, ReloadParams, SessionParams,
    TabParams, Viewport, ViewportParams, encode_browser_frame, is_local_url, network_status,
    preview_url, resolve_url, safe_file_name, truncate_chars,
};
use crate::browser_sessions::{InFlight, Sessions, Tab};
use crate::cdp::{Cdp, CdpEvent};
use crate::chrome::{self, DOWNLOADS_DIR, Instance, PROFILE_DIR};
use crate::connection::Responder;
use crate::protocol::{ErrorCode, Frame, RpcError};

/// Messages of the browser waiting for the relay's socket: pictures and events. Pictures are
/// few by themselves, since Chrome sends no more than two that are not acknowledged.
pub const BROWSER_QUEUE: usize = 64;

/// The isolated world the page scripts run in, out of sight of the page's own scripts.
const WORLD: &str = "hako";
const BINDING: &str = "hakoReport";
const PICKERS_SCRIPT: &str = include_str!("browser_pickers.js");
const WINDOW_OPEN_SCRIPT: &str = include_str!("browser_window_open.js");
const COPY_SCRIPT: &str = r#"(() => {
  const el = document.activeElement;
  try {
    if (el && typeof el.selectionStart === "number" && el.selectionEnd > el.selectionStart) {
      return el.value.slice(el.selectionStart, el.selectionEnd);
    }
  } catch (_) {}
  return String(getSelection());
})()"#;

const MAX_SESSIONS: usize = 8;
const JPEG_QUALITY: u32 = 70;
/// Windows are put this far right and down; macOS keeps a corner of them on the screen.
const OFF_SCREEN: i64 = 100_000;
const TICK: Duration = Duration::from_secs(1);
/// Chrome is quit after this long without a viewer.
const IDLE_QUIT: Duration = Duration::from_secs(15 * 60);
/// A session whose viewer acknowledges nothing for this long is closed.
const UNACKNOWLEDGED_CLOSE: Duration = Duration::from_secs(5 * 60);
/// A picture not acknowledged after this long was lost on the way.
const FRAME_LOST_AFTER: Duration = Duration::from_secs(15);
/// A page that has not changed for this long is sent again, which shows the viewer is still
/// there.
const KEEPALIVE_FRAME: Duration = Duration::from_secs(30);
/// A tab that appears with no opener is given this long to turn out to be one being created.
const UNOWNED_WAIT: Duration = Duration::from_secs(1);
const QUIT_WAIT: Duration = Duration::from_secs(10);
const DOWNLOADS_CLEAN_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Where the browser keeps its profile and downloads, and which previews it can open.
pub struct BrowserConfig {
    /// The app's data directory.
    pub data_dir: PathBuf,
    pub previews: PreviewAddresses,
}

pub(crate) enum Command {
    Request {
        method: BrowserMethod,
        params: Value,
        responder: Responder,
    },
    Notification {
        method: BrowserMethod,
        params: Value,
    },
    /// The relay connection dropped, and every session with it.
    Disconnected,
}

#[derive(Clone)]
pub(crate) struct BrowserHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl BrowserHandle {
    pub(crate) fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// Quits the browser when the app no longer wants it, even while the browser's thread is
/// stopping.
#[derive(Clone, Default)]
pub(crate) struct BrowserGuard(Arc<Mutex<Option<u32>>>);

impl BrowserGuard {
    fn set(&self, pid: Option<u32>) {
        if let Ok(mut current) = self.0.lock() {
            *current = pid;
        }
    }

    pub(crate) fn quit(&self) {
        let pid = self.0.lock().ok().and_then(|mut pid| pid.take());
        if let Some(pid) = pid {
            chrome::terminate(pid);
        }
    }
}

/// How the browser reaches the relay.
pub(crate) struct Out {
    pub(crate) queue: mpsc::Sender<Message>,
    /// Zero while there is no connection.
    pub(crate) epoch: Arc<AtomicU64>,
}

impl Out {
    fn connected(&self) -> bool {
        self.epoch.load(Ordering::Acquire) != 0
    }

    fn event<T: Serialize>(&self, name: &str, data: &T) {
        if !self.connected() {
            return;
        }
        let Ok(data) = serde_json::to_value(data) else {
            return;
        };
        if self
            .queue
            .try_send(Message::Text(Frame::event(name, data).encode()))
            .is_err()
        {
            log::debug!("[Mac browser] dropped the event {name}: the queue is full");
        }
    }

    /// Returns whether the picture was queued.
    fn frame(&self, frame: Vec<u8>) -> bool {
        self.connected() && self.queue.try_send(Message::Binary(frame)).is_ok()
    }
}

/// Starts the browser's bookkeeping. The future runs it until the handle is dropped.
pub(crate) fn start(
    config: BrowserConfig,
    out: Out,
) -> (
    BrowserHandle,
    BrowserGuard,
    impl std::future::Future<Output = ()>,
) {
    let (commands, received) = mpsc::unbounded_channel();
    let (internal, internal_received) = mpsc::unbounded_channel();
    let guard = BrowserGuard::default();
    let engine = Engine {
        config,
        out,
        guard: guard.clone(),
        internal,
        browser: Browser::Stopped,
        events: None,
        quitting: None,
        sessions: Sessions::default(),
        unowned: HashMap::new(),
        stale: HashSet::new(),
        created: HashMap::new(),
        creating: 0,
        pickers: HashMap::new(),
        choosers: HashMap::new(),
        downloads: HashMap::new(),
        next_id: 0,
        idle_since: None,
        downloads_cleaned: None,
    };
    (
        BrowserHandle { commands },
        guard,
        engine.run(received, internal_received),
    )
}

enum Browser {
    Stopped,
    Starting {
        waiting: Vec<PendingOpen>,
    },
    Running {
        cdp: Cdp,
        instance: Instance,
        user_agent: String,
    },
}

struct PendingOpen {
    url: String,
    viewport: Viewport,
    responder: Responder,
}

struct Started {
    cdp: Cdp,
    events: mpsc::UnboundedReceiver<CdpEvent>,
    instance: Instance,
    user_agent: String,
    /// Tabs of a browser that was found running, left from before the app last quit.
    stale: HashSet<String>,
}

/// What the engine's own tasks report back.
enum Internal {
    Started(Result<Started, RpcError>),
    Created {
        bid: u32,
        url: String,
        responder: Responder,
        target: Result<String, String>,
    },
    TabReady {
        target: String,
    },
    History {
        target: String,
        can_back: bool,
        can_forward: bool,
        title: Option<String>,
    },
    FileChooser {
        bid: u32,
        tab: String,
        multiple: bool,
        accept: String,
        backend_node: i64,
    },
    DownloadSaved {
        guid: String,
        path: PathBuf,
        size: i64,
    },
}

/// A page target that is attached and not yet given to a session.
struct Unowned {
    devtools_session: String,
    opener: Option<String>,
    waiting: bool,
    since: Instant,
    /// Left from before the app last quit, to be closed.
    stale: bool,
}

/// A tab created for a session, waiting for Chrome to attach to it.
struct Created {
    bid: u32,
    url: String,
    responder: Responder,
}

struct PickerTarget {
    bid: u32,
    devtools_session: String,
    context: i64,
    page_id: String,
}

struct ChooserTarget {
    bid: u32,
    devtools_session: String,
    backend_node: i64,
}

struct Download {
    bid: u32,
    name: String,
    /// Where the file is kept once it has arrived.
    destination: PathBuf,
}

struct Engine {
    config: BrowserConfig,
    out: Out,
    guard: BrowserGuard,
    internal: mpsc::UnboundedSender<Internal>,
    browser: Browser,
    events: Option<mpsc::UnboundedReceiver<CdpEvent>>,
    /// A browser that was asked to quit and may still be running.
    quitting: Option<u32>,
    sessions: Sessions,
    unowned: HashMap<String, Unowned>,
    stale: HashSet<String>,
    created: HashMap<String, Created>,
    /// Tabs being created for sessions that Chrome has not named yet.
    creating: usize,
    pickers: HashMap<String, PickerTarget>,
    choosers: HashMap<String, ChooserTarget>,
    downloads: HashMap<String, Download>,
    next_id: u64,
    idle_since: Option<Instant>,
    downloads_cleaned: Option<Instant>,
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|error| RpcError::bad_request(format!("Invalid params: {error}")))
}

fn no_session(bid: u32) -> RpcError {
    RpcError::not_found(format!("There is no browser session {bid}"))
}

fn to_json<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| RpcError::failed(error.to_string()))
}

/// The DevTools commands that show a page as `viewport` describes.
fn emulation(viewport: &Viewport, default_user_agent: &str) -> Vec<(&'static str, Value)> {
    let (orientation, angle) = if viewport.landscape() {
        ("landscapePrimary", 90)
    } else {
        ("portraitPrimary", 0)
    };
    let touch = if viewport.touch {
        json!({ "enabled": true, "maxTouchPoints": 5 })
    } else {
        json!({ "enabled": false })
    };
    vec![
        (
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": viewport.width,
                "height": viewport.height,
                "deviceScaleFactor": viewport.scale,
                "mobile": viewport.mobile,
                "screenOrientation": { "type": orientation, "angle": angle },
            }),
        ),
        ("Emulation.setTouchEmulationEnabled", touch),
        (
            "Emulation.setUserAgentOverride",
            json!({ "userAgent": viewport.user_agent.as_deref().unwrap_or(default_user_agent) }),
        ),
    ]
}

impl Engine {
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut internal: mpsc::UnboundedReceiver<Internal>,
    ) {
        let mut tick = interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command),
                    None => break,
                },
                Some(message) = internal.recv() => self.internal(message),
                event = next_event(&mut self.events) => match event {
                    Some(event) => self.devtools_event(event),
                    None => self.browser_exited(),
                },
                _ = tick.tick() => self.tick(),
            }
        }
        self.quit_browser();
    }

    fn cdp(&self) -> Option<&Cdp> {
        match &self.browser {
            Browser::Running { cdp, .. } => Some(cdp),
            Browser::Stopped | Browser::Starting { .. } => None,
        }
    }

    fn next_id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Request {
                method,
                params,
                responder,
            } => self.request(method, params, responder),
            Command::Notification { method, params } => self.notification(method, params),
            Command::Disconnected => {
                for bid in self.sessions.bids() {
                    self.close_session(bid, None);
                }
            }
        }
    }

    fn request(&mut self, method: BrowserMethod, params: Value, responder: Responder) {
        let outcome = match method {
            BrowserMethod::Open => return self.open(params, responder),
            BrowserMethod::Close => parse(params).and_then(|SessionParams { bid }| {
                self.close_session(bid, None)
                    .then(|| json!({}))
                    .ok_or_else(|| no_session(bid))
            }),
            BrowserMethod::Navigate => parse(params).and_then(|params: NavigateParams| {
                let url = resolve_url(&params.url)?;
                self.on_active_tab(params.bid, "Page.navigate", json!({ "url": url }))
            }),
            BrowserMethod::Back | BrowserMethod::Forward => {
                return self.go_through_history(method == BrowserMethod::Back, params, responder);
            }
            BrowserMethod::Reload => parse(params).and_then(|params: ReloadParams| {
                self.on_active_tab(
                    params.bid,
                    "Page.reload",
                    json!({ "ignoreCache": params.hard }),
                )
            }),
            BrowserMethod::StopLoading => parse(params).and_then(|SessionParams { bid }| {
                self.on_active_tab(bid, "Page.stopLoading", json!({}))
            }),
            BrowserMethod::Viewport => {
                parse(params).and_then(|params: ViewportParams| self.change_viewport(params))
            }
            BrowserMethod::SwitchTab => parse(params).and_then(|params: TabParams| {
                self.switch_tab(params.bid, &params.tab).map(|()| json!({}))
            }),
            BrowserMethod::CloseTab => parse(params).and_then(|params: TabParams| {
                let owned = self
                    .sessions
                    .get(params.bid)
                    .ok_or_else(|| no_session(params.bid))?
                    .tab(&params.tab)
                    .is_some();
                if !owned {
                    return Err(RpcError::not_found("The session has no such tab"));
                }
                if let Some(cdp) = self.cdp() {
                    cdp.send(
                        None,
                        "Target.closeTarget",
                        json!({ "targetId": params.tab }),
                    );
                }
                Ok(json!({}))
            }),
            BrowserMethod::DialogReply => return self.reply_to_dialog(params, responder),
            BrowserMethod::PickerReply => return self.reply_to_picker(params, responder),
            BrowserMethod::FileChooserReply => {
                return self.reply_to_file_chooser(params, responder);
            }
            BrowserMethod::Copy => return self.copy(params, responder),
            BrowserMethod::Ack | BrowserMethod::Input => Err(RpcError::bad_request(
                "This is a notification, not a request",
            )),
        };
        responder.respond(outcome);
    }

    fn notification(&mut self, method: BrowserMethod, params: Value) {
        match method {
            BrowserMethod::Ack => match parse::<AckParams>(params) {
                Ok(AckParams { bid, frame }) => {
                    let now = Instant::now();
                    let acknowledged = self
                        .sessions
                        .get_mut(bid)
                        .map(|session| {
                            session.last_used = now;
                            session.acknowledged(frame, now)
                        })
                        .unwrap_or_default();
                    self.acknowledge(acknowledged);
                }
                Err(error) => log::debug!("[Mac browser] ignoring an ack: {}", error.message),
            },
            BrowserMethod::Input => match parse::<InputParams>(params) {
                Ok(params) => self.input(params),
                Err(error) => log::debug!("[Mac browser] ignoring input: {}", error.message),
            },
            BrowserMethod::Open
            | BrowserMethod::Close
            | BrowserMethod::Navigate
            | BrowserMethod::Back
            | BrowserMethod::Forward
            | BrowserMethod::Reload
            | BrowserMethod::StopLoading
            | BrowserMethod::Viewport
            | BrowserMethod::SwitchTab
            | BrowserMethod::CloseTab
            | BrowserMethod::DialogReply
            | BrowserMethod::PickerReply
            | BrowserMethod::FileChooserReply
            | BrowserMethod::Copy => {
                log::debug!("[Mac browser] ignoring a request sent as a notification");
            }
        }
    }

    fn input(&mut self, params: InputParams) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(params.bid) else {
            return;
        };
        let now = Instant::now();
        session.last_used = now;
        let Some(target) = session.active_tab().map(|tab| tab.devtools_session.clone()) else {
            return;
        };
        let touch = session.viewport.touch;
        for event in params.events {
            for call in session.input.translate(event, touch, now) {
                cdp.send(Some(&target), call.method, call.params);
            }
        }
    }

    fn acknowledge(&self, frames: Vec<InFlight>) {
        let Some(cdp) = self.cdp() else {
            return;
        };
        for frame in frames {
            cdp.send(
                Some(&frame.devtools_session),
                "Page.screencastFrameAck",
                json!({ "sessionId": frame.screencast }),
            );
        }
    }

    fn on_active_tab(&mut self, bid: u32, method: &str, params: Value) -> Result<Value, RpcError> {
        let session = self.sessions.get_mut(bid).ok_or_else(|| no_session(bid))?;
        session.last_used = Instant::now();
        let target = session
            .active_tab()
            .map(|tab| tab.devtools_session.clone())
            .ok_or_else(|| RpcError::not_found("The session has no tab"))?;
        if let Some(cdp) = self.cdp() {
            cdp.send(Some(&target), method, params);
        }
        Ok(json!({}))
    }

    fn open(&mut self, params: Value, responder: Responder) {
        let opening = parse::<OpenParams>(params).and_then(|params| {
            params.viewport.validate()?;
            let url = match &params.preview_id {
                Some(id) => {
                    let address =
                        self.config.previews.get(id).ok_or_else(|| {
                            RpcError::not_found(format!("No preview called {id}"))
                        })?;
                    preview_url(&address, params.url.as_deref())?
                }
                None => resolve_url(
                    params
                        .url
                        .as_deref()
                        .ok_or_else(|| RpcError::bad_request("Give a url or a preview_id"))?,
                )?,
            };
            if self.sessions.len() >= MAX_SESSIONS {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "Too many browser sessions are open; close one first",
                ));
            }
            Ok((url, params.viewport))
        });
        let (url, viewport) = match opening {
            Ok(opening) => opening,
            Err(error) => return responder.respond(Err(error)),
        };
        let pending = PendingOpen {
            url,
            viewport,
            responder,
        };
        match &mut self.browser {
            Browser::Running { .. } => self.create_session(pending),
            Browser::Starting { waiting } => waiting.push(pending),
            Browser::Stopped => self.start_browser(pending),
        }
    }

    fn start_browser(&mut self, first: PendingOpen) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let Some(app) = chrome::find_app(&home, Path::exists) else {
            return first.responder.respond(Err(RpcError::failed(
                "Google Chrome is not installed on this Mac; install it to use the Mac browser",
            )));
        };
        self.browser = Browser::Starting {
            waiting: vec![first],
        };
        let profile = self.config.data_dir.join(PROFILE_DIR);
        let downloads = self.config.data_dir.join(DOWNLOADS_DIR);
        let quitting = self.quitting.take();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            if let Some(pid) = quitting {
                let deadline = Instant::now() + QUIT_WAIT;
                while chrome::is_running(pid) && Instant::now() < deadline {
                    sleep(Duration::from_millis(200)).await;
                }
            }
            let started = launch(&app, &profile, &downloads).await;
            let _ = internal.send(Internal::Started(started));
        });
    }

    fn create_session(&mut self, pending: PendingOpen) {
        let Some(cdp) = self.cdp().cloned() else {
            return pending
                .responder
                .respond(Err(RpcError::failed("The browser is not running")));
        };
        let bid = self.sessions.open(pending.viewport, Instant::now());
        self.idle_since = None;
        self.creating += 1;
        let internal = self.internal.clone();
        tokio::spawn(async move {
            let created = cdp
                .call(
                    None,
                    "Target.createTarget",
                    json!({
                        "url": "about:blank",
                        "newWindow": true,
                        "background": true,
                        "left": OFF_SCREEN,
                        "top": OFF_SCREEN,
                        "width": 500,
                        "height": 800,
                    }),
                )
                .await
                .and_then(|result| {
                    result["targetId"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "Chrome did not name the new tab".to_owned())
                });
            let _ = internal.send(Internal::Created {
                bid,
                url: pending.url,
                responder: pending.responder,
                target: created,
            });
        });
    }

    fn internal(&mut self, message: Internal) {
        match message {
            Internal::Started(started) => self.started(started),
            Internal::Created {
                bid,
                url,
                responder,
                target,
            } => {
                self.creating = self.creating.saturating_sub(1);
                let target = match target {
                    Ok(target) => target,
                    Err(error) => {
                        self.sessions.close(bid);
                        return responder.respond(Err(RpcError::failed(format!(
                            "Chrome could not open a tab: {error}"
                        ))));
                    }
                };
                if self.sessions.get(bid).is_none() {
                    if let Some(cdp) = self.cdp() {
                        cdp.send(None, "Target.closeTarget", json!({ "targetId": target }));
                    }
                    return responder.respond(Err(no_session(bid)));
                }
                match self.unowned.remove(&target) {
                    Some(unowned) => {
                        self.set_up_tab(bid, target, unowned, Some(url), true, Some(responder));
                    }
                    None => {
                        self.created.insert(
                            target,
                            Created {
                                bid,
                                url,
                                responder,
                            },
                        );
                    }
                }
            }
            Internal::TabReady { target } => {
                let Some((bid, tab)) = self.sessions.tab_mut(&target) else {
                    return;
                };
                tab.ready = true;
                if self.sessions.is_active(&target) {
                    self.show_active_tab(bid);
                }
                self.refresh_history(&target);
                self.send_state(bid);
            }
            Internal::History {
                target,
                can_back,
                can_forward,
                title,
            } => {
                let Some((bid, tab)) = self.sessions.tab_mut(&target) else {
                    return;
                };
                let title = title.unwrap_or_else(|| tab.title.clone());
                if (tab.can_back, tab.can_forward, &tab.title) != (can_back, can_forward, &title) {
                    tab.can_back = can_back;
                    tab.can_forward = can_forward;
                    tab.title = title;
                    self.send_state(bid);
                }
            }
            Internal::FileChooser {
                bid,
                tab,
                multiple,
                accept,
                backend_node,
            } => {
                let Some(devtools_session) = self
                    .sessions
                    .get(bid)
                    .and_then(|session| session.tab(&tab))
                    .map(|tab| tab.devtools_session.clone())
                else {
                    return;
                };
                let id = self.next_id();
                self.choosers.insert(
                    id.clone(),
                    ChooserTarget {
                        bid,
                        devtools_session,
                        backend_node,
                    },
                );
                self.out.event(
                    EVENT_FILE_CHOOSER,
                    &FileChooserEvent {
                        bid,
                        tab,
                        id,
                        multiple,
                        accept,
                    },
                );
            }
            Internal::DownloadSaved { guid, path, size } => {
                let Some(download) = self.downloads.remove(&guid) else {
                    return;
                };
                self.out.event(
                    EVENT_DOWNLOAD,
                    &DownloadEvent {
                        bid: download.bid,
                        name: download.name,
                        path: path.display().to_string(),
                        size,
                        done: true,
                    },
                );
            }
        }
    }

    fn started(&mut self, started: Result<Started, RpcError>) {
        let waiting = match std::mem::replace(&mut self.browser, Browser::Stopped) {
            Browser::Starting { waiting } => waiting,
            Browser::Stopped | Browser::Running { .. } => Vec::new(),
        };
        match started {
            Ok(started) => {
                log::info!(
                    "[Mac browser] Chrome runs as pid {} on port {}",
                    started.instance.pid,
                    started.instance.port
                );
                self.guard.set(Some(started.instance.pid));
                self.events = Some(started.events);
                self.stale = started.stale;
                self.browser = Browser::Running {
                    cdp: started.cdp,
                    instance: started.instance,
                    user_agent: started.user_agent,
                };
                for pending in waiting {
                    self.create_session(pending);
                }
            }
            Err(error) => {
                log::warn!(
                    "[Mac browser] Chrome could not be started: {}",
                    error.message
                );
                for pending in waiting {
                    pending.responder.respond(Err(error.clone()));
                }
            }
        }
    }

    fn answer_open(&self, bid: u32, responder: Responder) {
        let Some(session) = self.sessions.get(bid) else {
            return responder.respond(Err(no_session(bid)));
        };
        responder.respond(to_json(&OpenResult {
            bid,
            tabs: session.tab_infos(),
        }));
    }

    /// Gives an attached tab to a session and sets it up: pictures and pickers it may need, the
    /// session's viewport, and its window off the screen. It is opened at `url` when given.
    /// `open` is the `browser.open` that made the session, answered before anything else of the
    /// session is sent.
    fn set_up_tab(
        &mut self,
        bid: u32,
        target: String,
        unowned: Unowned,
        url: Option<String>,
        activate: bool,
        open: Option<Responder>,
    ) {
        let Some(cdp) = self.cdp().cloned() else {
            if let Some(open) = open {
                open.respond(Err(RpcError::failed("The browser is not running")));
            }
            return;
        };
        let user_agent = match &self.browser {
            Browser::Running { user_agent, .. } => user_agent.clone(),
            Browser::Stopped | Browser::Starting { .. } => String::new(),
        };
        let Some(viewport) = self
            .sessions
            .get(bid)
            .map(|session| session.viewport.clone())
        else {
            if let Some(open) = open {
                open.respond(Err(no_session(bid)));
            }
            return;
        };
        let session = unowned.devtools_session.clone();
        let waiting = unowned.waiting;
        let previous = self
            .sessions
            .get(bid)
            .and_then(|session| session.active_tab().map(|tab| tab.id.clone()));
        let tab = Tab::new(target.clone(), session.clone(), unowned.opener);
        self.sessions.add_tab(bid, tab, activate);
        if let Some(open) = open {
            self.answer_open(bid, open);
        }
        if activate {
            if let Some(previous) = previous.filter(|previous| *previous != target) {
                self.stop_screencast(bid, &previous);
            }
        }
        self.send_state(bid);
        let internal = self.internal.clone();
        tokio::spawn(async move {
            let page = Some(session.as_str());
            // A tab waiting for the debugger may answer none of these until it runs, so all of
            // them go out before it is let go, and only then is anything waited for.
            cdp.send(page, "Page.enable", json!({}));
            cdp.send(
                page,
                "Page.setInterceptFileChooserDialog",
                json!({ "enabled": true }),
            );
            cdp.send(
                page,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": WINDOW_OPEN_SCRIPT }),
            );
            for (method, params) in emulation(&viewport, &user_agent) {
                cdp.send(page, method, params);
            }
            if waiting {
                cdp.send(page, "Runtime.runIfWaitingForDebugger", json!({}));
            }
            if let Ok(window) = cdp
                .call(
                    None,
                    "Browser.getWindowForTarget",
                    json!({ "targetId": target }),
                )
                .await
            {
                cdp.send(
                    None,
                    "Browser.setWindowBounds",
                    json!({
                        "windowId": window["windowId"],
                        "bounds": { "left": OFF_SCREEN, "top": OFF_SCREEN },
                    }),
                );
            }
            if let Some(url) = url {
                cdp.send(page, "Page.navigate", json!({ "url": url }));
            }
            // Answered after everything before it, so the tab is set up by then.
            let _ = cdp.call(page, "Page.getFrameTree", json!({})).await;
            let _ = internal.send(Internal::TabReady { target });
        });
    }

    /// Brings the session's active tab to the front of its window, which only draws its front
    /// tab, shows it as the session's viewport again, which Chrome forgets for a tab that was in
    /// the background, and starts sending its pictures.
    fn show_active_tab(&mut self, bid: u32) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let user_agent = match &self.browser {
            Browser::Running { user_agent, .. } => user_agent.clone(),
            Browser::Stopped | Browser::Starting { .. } => String::new(),
        };
        let Some(session) = self.sessions.get(bid) else {
            return;
        };
        let Some(tab) = session.active_tab().filter(|tab| tab.ready) else {
            return;
        };
        // Bringing a tab forward does not take the focus from the owner's app while Chrome is
        // in the background and not hidden.
        cdp.send(None, "Target.activateTarget", json!({ "targetId": tab.id }));
        for (method, params) in emulation(&session.viewport, &user_agent) {
            cdp.send(Some(&tab.devtools_session), method, params);
        }
        self.start_screencast(bid);
    }

    fn start_screencast(&mut self, bid: u32) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(bid) else {
            return;
        };
        let Some(tab) = session.active_tab().filter(|tab| tab.ready) else {
            return;
        };
        let devtools_session = tab.devtools_session.clone();
        let (width, height) = session.viewport.image_size();
        session.last_frame = Instant::now();
        cdp.send(
            Some(&devtools_session),
            "Page.startScreencast",
            json!({
                "format": "jpeg",
                "quality": JPEG_QUALITY,
                "maxWidth": width,
                "maxHeight": height,
                "everyNthFrame": 1,
            }),
        );
    }

    fn stop_screencast(&mut self, bid: u32, tab: &str) {
        let Some(devtools_session) = self
            .sessions
            .get(bid)
            .and_then(|session| session.tab(tab))
            .map(|tab| tab.devtools_session.clone())
        else {
            return;
        };
        if let Some(cdp) = self.cdp() {
            cdp.send(Some(&devtools_session), "Page.stopScreencast", json!({}));
        }
        let released = self
            .sessions
            .get_mut(bid)
            .map(|session| session.release_all())
            .unwrap_or_default();
        self.acknowledge(released);
    }

    fn switch_tab(&mut self, bid: u32, tab: &str) -> Result<(), RpcError> {
        let session = self.sessions.get_mut(bid).ok_or_else(|| no_session(bid))?;
        session.last_used = Instant::now();
        let previous = session
            .activate(tab)
            .ok_or_else(|| RpcError::not_found("The session has no such tab"))?;
        if previous.as_deref() == Some(tab) {
            return Ok(());
        }
        if let Some(previous) = previous {
            self.stop_screencast(bid, &previous);
        }
        self.show_active_tab(bid);
        self.refresh_history(tab);
        self.send_state(bid);
        Ok(())
    }

    fn change_viewport(&mut self, params: ViewportParams) -> Result<Value, RpcError> {
        params.viewport.validate()?;
        let user_agent = match &self.browser {
            Browser::Running { user_agent, .. } => user_agent.clone(),
            Browser::Stopped | Browser::Starting { .. } => String::new(),
        };
        let session = self
            .sessions
            .get_mut(params.bid)
            .ok_or_else(|| no_session(params.bid))?;
        session.last_used = Instant::now();
        session.viewport = params.viewport;
        let calls = emulation(&session.viewport, &user_agent);
        let tabs: Vec<String> = session
            .tabs()
            .iter()
            .map(|tab| tab.devtools_session.clone())
            .collect();
        let active = session.active_tab().map(|tab| tab.id.clone());
        if let Some(cdp) = self.cdp() {
            for tab in &tabs {
                for (method, params) in &calls {
                    cdp.send(Some(tab), method, params.clone());
                }
            }
        }
        // The pictures' size limit changes with the viewport.
        if let Some(active) = active {
            self.stop_screencast(params.bid, &active);
            self.start_screencast(params.bid);
        }
        Ok(json!({}))
    }

    fn go_through_history(&mut self, back: bool, params: Value, responder: Responder) {
        let target = parse::<SessionParams>(params).and_then(|SessionParams { bid }| {
            let session = self.sessions.get_mut(bid).ok_or_else(|| no_session(bid))?;
            session.last_used = Instant::now();
            session
                .active_tab()
                .map(|tab| tab.devtools_session.clone())
                .ok_or_else(|| RpcError::not_found("The session has no tab"))
        });
        let (target, cdp) = match (target, self.cdp().cloned()) {
            (Ok(target), Some(cdp)) => (target, cdp),
            (Err(error), _) => return responder.respond(Err(error)),
            (Ok(_), None) => {
                return responder.respond(Err(RpcError::failed("The browser is not running")));
            }
        };
        tokio::spawn(async move {
            let page = Some(target.as_str());
            let outcome = match cdp.call(page, "Page.getNavigationHistory", json!({})).await {
                Ok(history) => {
                    let moves = HistoryMoves::of(&history);
                    let entry = if back { moves.back } else { moves.forward };
                    if let Some(entry) = entry {
                        cdp.send(
                            page,
                            "Page.navigateToHistoryEntry",
                            json!({ "entryId": entry }),
                        );
                    }
                    Ok(json!({}))
                }
                Err(error) => Err(RpcError::failed(error)),
            };
            responder.respond(outcome);
        });
    }

    fn refresh_history(&self, target: &str) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let Some(devtools_session) = self
            .sessions
            .owner(target)
            .and_then(|bid| self.sessions.get(bid))
            .and_then(|session| session.tab(target))
            .map(|tab| tab.devtools_session.clone())
        else {
            return;
        };
        let internal = self.internal.clone();
        let target = target.to_owned();
        tokio::spawn(async move {
            let Ok(history) = cdp
                .call(
                    Some(&devtools_session),
                    "Page.getNavigationHistory",
                    json!({}),
                )
                .await
            else {
                return;
            };
            let moves = HistoryMoves::of(&history);
            let _ = internal.send(Internal::History {
                target,
                can_back: moves.back.is_some(),
                can_forward: moves.forward.is_some(),
                title: moves.title,
            });
        });
    }

    fn reply_to_dialog(&mut self, params: Value, responder: Responder) {
        let target = parse::<DialogReplyParams>(params).and_then(|params| {
            let session = self
                .sessions
                .get_mut(params.bid)
                .ok_or_else(|| no_session(params.bid))?;
            let showing = session.dialog.take();
            let tab = match params.tab.as_ref().or(showing.as_ref()) {
                Some(tab) => session.tab(tab),
                None => session.active_tab(),
            }
            .ok_or_else(|| RpcError::not_found("The session has no such tab"))?;
            Ok((tab.devtools_session.clone(), params))
        });
        let ((target, params), cdp) = match (target, self.cdp().cloned()) {
            (Ok(target), Some(cdp)) => (target, cdp),
            (Err(error), _) => return responder.respond(Err(error)),
            (Ok(_), None) => {
                return responder.respond(Err(RpcError::failed("The browser is not running")));
            }
        };
        let mut answer = json!({ "accept": params.accept });
        if let Some(text) = params.text {
            answer["promptText"] = json!(text);
        }
        tokio::spawn(async move {
            let outcome = cdp
                .call(Some(&target), "Page.handleJavaScriptDialog", answer)
                .await
                .map(|_| json!({}))
                .map_err(|_| RpcError::not_found("No dialog is open in that tab"));
            responder.respond(outcome);
        });
    }

    fn reply_to_picker(&mut self, params: Value, responder: Responder) {
        let picked = parse::<PickerReplyParams>(params).and_then(|params| {
            match self.pickers.remove(&params.id) {
                Some(picker) if picker.bid == params.bid => Ok((picker, params.values)),
                Some(_) | None => Err(RpcError::not_found("The picker is no longer open")),
            }
        });
        let ((picker, values), cdp) = match (picked, self.cdp().cloned()) {
            (Ok(picked), Some(cdp)) => (picked, cdp),
            (Err(error), _) => return responder.respond(Err(error)),
            (Ok(_), None) => {
                return responder.respond(Err(RpcError::failed("The browser is not running")));
            }
        };
        let expression = format!("hakoPick({}, {})", json!(picker.page_id), json!(values));
        tokio::spawn(async move {
            let outcome = cdp
                .call(
                    Some(&picker.devtools_session),
                    "Runtime.evaluate",
                    json!({ "expression": expression, "contextId": picker.context, "returnByValue": true }),
                )
                .await;
            responder.respond(match outcome {
                Ok(result) if result["result"]["value"] == json!(true) => Ok(json!({})),
                Ok(_) | Err(_) => Err(RpcError::not_found(
                    "The picker's field is gone from the page",
                )),
            });
        });
    }

    fn reply_to_file_chooser(&mut self, params: Value, responder: Responder) {
        let chosen = parse::<FileChooserReplyParams>(params).and_then(|params| {
            let chooser = match self.choosers.remove(&params.id) {
                Some(chooser) if chooser.bid == params.bid => chooser,
                Some(_) | None => {
                    return Err(RpcError::not_found("The file chooser is no longer open"));
                }
            };
            for path in &params.paths {
                let path = Path::new(path);
                if !path.is_absolute() || !path.is_file() {
                    return Err(RpcError::not_found(format!(
                        "No file at {}",
                        path.display()
                    )));
                }
            }
            Ok((chooser, params.paths))
        });
        let ((chooser, paths), cdp) = match (chosen, self.cdp().cloned()) {
            (Ok(chosen), Some(cdp)) => (chosen, cdp),
            (Err(error), _) => return responder.respond(Err(error)),
            (Ok(_), None) => {
                return responder.respond(Err(RpcError::failed("The browser is not running")));
            }
        };
        if paths.is_empty() {
            return responder.respond(Ok(json!({})));
        }
        tokio::spawn(async move {
            let outcome = cdp
                .call(
                    Some(&chooser.devtools_session),
                    "DOM.setFileInputFiles",
                    json!({ "files": paths, "backendNodeId": chooser.backend_node }),
                )
                .await
                .map(|_| json!({}))
                .map_err(|error| {
                    RpcError::failed(format!("The files could not be chosen: {error}"))
                });
            responder.respond(outcome);
        });
    }

    fn copy(&mut self, params: Value, responder: Responder) {
        let target = parse::<SessionParams>(params).and_then(|SessionParams { bid }| {
            let session = self.sessions.get(bid).ok_or_else(|| no_session(bid))?;
            let tab = session
                .active_tab()
                .ok_or_else(|| RpcError::not_found("The session has no tab"))?;
            if !tab.local {
                return Err(RpcError::failed(
                    "Copying works on pages of this Mac's dev servers only",
                ));
            }
            Ok((tab.id.clone(), tab.devtools_session.clone()))
        });
        let ((frame, target), cdp) = match (target, self.cdp().cloned()) {
            (Ok(target), Some(cdp)) => (target, cdp),
            (Err(error), _) => return responder.respond(Err(error)),
            (Ok(_), None) => {
                return responder.respond(Err(RpcError::failed("The browser is not running")));
            }
        };
        tokio::spawn(async move {
            let page = Some(target.as_str());
            let outcome = async {
                let world = cdp
                    .call(
                        page,
                        "Page.createIsolatedWorld",
                        json!({ "frameId": frame, "worldName": "hako-copy" }),
                    )
                    .await?;
                let result = cdp
                    .call(
                        page,
                        "Runtime.evaluate",
                        json!({
                            "expression": COPY_SCRIPT,
                            "contextId": world["executionContextId"],
                            "returnByValue": true,
                        }),
                    )
                    .await?;
                Ok::<_, String>(
                    result["result"]["value"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
            }
            .await;
            responder.respond(
                outcome
                    .map_err(RpcError::failed)
                    .and_then(|text| to_json(&CopyResult { text })),
            );
        });
    }

    /// Ends a session and closes its tabs. Returns false when there is no such session.
    fn close_session(&mut self, bid: u32, reason: Option<ClosedReason>) -> bool {
        let Some(mut session) = self.sessions.close(bid) else {
            return false;
        };
        let released = session.release_all();
        self.acknowledge(released);
        if let Some(cdp) = self.cdp() {
            for tab in session.tabs() {
                cdp.send(None, "Target.closeTarget", json!({ "targetId": tab.id }));
            }
        }
        self.pickers.retain(|_, picker| picker.bid != bid);
        self.choosers.retain(|_, chooser| chooser.bid != bid);
        if let Some(reason) = reason {
            self.out.event(EVENT_CLOSED, &ClosedEvent::new(bid, reason));
        }
        if self.sessions.is_empty() {
            self.idle_since = Some(Instant::now());
        }
        true
    }

    fn send_state(&self, bid: u32) {
        if let Some(session) = self.sessions.get(bid) {
            self.out.event(EVENT_STATE, &session.state(bid));
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.creating == 0 {
            let due: Vec<String> = self
                .unowned
                .iter()
                .filter(|(_, unowned)| now.saturating_duration_since(unowned.since) >= UNOWNED_WAIT)
                .map(|(target, _)| target.clone())
                .collect();
            for target in due {
                let Some(unowned) = self.unowned.remove(&target) else {
                    continue;
                };
                // A browser whose last window closes quits, so stale tabs go once the new ones are
                // there.
                match self
                    .sessions
                    .most_recently_used()
                    .filter(|_| !unowned.stale)
                {
                    Some(bid) => self.set_up_tab(bid, target, unowned, None, true, None),
                    None => {
                        if let Some(cdp) = self.cdp() {
                            cdp.send(None, "Target.closeTarget", json!({ "targetId": target }));
                        }
                    }
                }
            }
        }

        if let Some(since) = now.checked_sub(UNACKNOWLEDGED_CLOSE) {
            for bid in self.sessions.unacknowledged_since(since) {
                log::info!(
                    "[Mac browser] closing session {bid}, which acknowledged nothing for a while"
                );
                self.close_session(bid, Some(ClosedReason::Unacknowledged));
            }
        }

        let mut lost = Vec::new();
        let mut quiet = Vec::new();
        for (bid, session) in self.sessions.iter_mut() {
            if let Some(before) = now.checked_sub(FRAME_LOST_AFTER) {
                lost.extend(session.release_sent_before(before));
            }
            if session.in_flight() == 0
                && now.saturating_duration_since(session.last_frame) >= KEEPALIVE_FRAME
            {
                quiet.push(bid);
            }
        }
        self.acknowledge(lost);
        for bid in quiet {
            if let Some(active) = self
                .sessions
                .get(bid)
                .and_then(|session| session.active_tab())
                .map(|tab| tab.id.clone())
            {
                self.stop_screencast(bid, &active);
                self.start_screencast(bid);
            }
        }

        let idle = self
            .idle_since
            .is_some_and(|since| now.saturating_duration_since(since) >= IDLE_QUIT);
        if idle && self.sessions.is_empty() && matches!(self.browser, Browser::Running { .. }) {
            log::info!("[Mac browser] quitting Chrome, which has had no viewer for a while");
            self.quit_browser();
        }

        let clean_due = self.downloads_cleaned.is_none_or(|cleaned| {
            now.saturating_duration_since(cleaned) >= DOWNLOADS_CLEAN_INTERVAL
        });
        if clean_due && matches!(self.browser, Browser::Running { .. }) {
            self.downloads_cleaned = Some(now);
            let downloads = self.config.data_dir.join(DOWNLOADS_DIR);
            tokio::task::spawn_blocking(move || {
                chrome::clean_downloads(&downloads, SystemTime::now())
            });
        }
    }

    fn quit_browser(&mut self) {
        let Browser::Running { cdp, instance, .. } =
            std::mem::replace(&mut self.browser, Browser::Stopped)
        else {
            return;
        };
        self.events = None;
        self.guard.set(None);
        self.quitting = Some(instance.pid);
        self.forget_everything(None);
        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                cdp.call(None, "Browser.close", json!({})),
            )
            .await;
            let deadline = Instant::now() + Duration::from_secs(5);
            while chrome::is_running(instance.pid) && Instant::now() < deadline {
                sleep(Duration::from_millis(200)).await;
            }
            if chrome::is_running(instance.pid) {
                chrome::terminate(instance.pid);
            }
        });
    }

    fn browser_exited(&mut self) {
        log::info!("[Mac browser] Chrome is gone");
        self.events = None;
        self.guard.set(None);
        self.browser = Browser::Stopped;
        self.forget_everything(Some(ClosedReason::BrowserExited));
    }

    fn forget_everything(&mut self, reason: Option<ClosedReason>) {
        for bid in self.sessions.bids() {
            self.sessions.close(bid);
            if let Some(reason) = reason {
                self.out.event(EVENT_CLOSED, &ClosedEvent::new(bid, reason));
            }
        }
        for (_, created) in self.created.drain() {
            created
                .responder
                .respond(Err(RpcError::failed("The browser quit")));
        }
        self.unowned.clear();
        self.stale.clear();
        self.pickers.clear();
        self.choosers.clear();
        self.downloads.clear();
        self.creating = 0;
        self.idle_since = None;
    }

    fn devtools_event(&mut self, event: CdpEvent) {
        match event.session {
            None => self.browser_event(&event.method, event.params),
            Some(session) => self.page_event(&session, &event.method, event.params),
        }
    }

    fn browser_event(&mut self, method: &str, params: Value) {
        match method {
            "Target.attachedToTarget" => {
                let info = &params["targetInfo"];
                let (Some(target), Some(session)) =
                    (info["targetId"].as_str(), params["sessionId"].as_str())
                else {
                    return;
                };
                if info["type"].as_str() != Some("page") {
                    return;
                }
                let target = target.to_owned();
                let opener = info["openerId"].as_str().map(str::to_owned);
                let unowned = Unowned {
                    devtools_session: session.to_owned(),
                    opener: opener.clone(),
                    waiting: params["waitingForDebugger"].as_bool().unwrap_or(false),
                    since: Instant::now(),
                    stale: self.stale.remove(&target),
                };
                if unowned.stale {
                    self.unowned.insert(target, unowned);
                } else if let Some(created) = self.created.remove(&target) {
                    self.set_up_tab(
                        created.bid,
                        target,
                        unowned,
                        Some(created.url),
                        true,
                        Some(created.responder),
                    );
                } else if let Some(bid) = opener
                    .as_deref()
                    .and_then(|opener| self.sessions.owner(opener))
                {
                    // A pop-up or a new tab the page opened, shown in place of its opener.
                    self.set_up_tab(bid, target, unowned, None, true, None);
                } else {
                    self.unowned.insert(target, unowned);
                }
            }
            "Target.targetDestroyed" | "Target.detachedFromTarget" => {
                let Some(target) = params["targetId"].as_str() else {
                    return;
                };
                self.unowned.remove(target);
                let Some(removed) = self.sessions.remove_tab(target) else {
                    return;
                };
                self.pickers
                    .retain(|_, picker| picker.devtools_session != removed.tab.devtools_session);
                self.choosers
                    .retain(|_, chooser| chooser.devtools_session != removed.tab.devtools_session);
                if removed.empty {
                    self.close_session(removed.bid, Some(ClosedReason::TabsClosed));
                    return;
                }
                if let Some(activated) = removed.activated {
                    let released = self
                        .sessions
                        .get_mut(removed.bid)
                        .map(|session| session.release_all())
                        .unwrap_or_default();
                    self.acknowledge(released);
                    self.show_active_tab(removed.bid);
                    self.refresh_history(&activated);
                }
                self.send_state(removed.bid);
            }
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                let Some(target) = info["targetId"].as_str() else {
                    return;
                };
                let Some((bid, tab)) = self.sessions.tab_mut(target) else {
                    return;
                };
                let title = info["title"].as_str().unwrap_or_default();
                let url = info["url"].as_str().unwrap_or_default();
                if tab.title != title || (!url.is_empty() && tab.url != url) {
                    tab.title = title.to_owned();
                    if !url.is_empty() {
                        tab.url = url.to_owned();
                    }
                    self.send_state(bid);
                }
            }
            "Browser.downloadWillBegin" => {
                let Some(guid) = params["guid"].as_str() else {
                    return;
                };
                let frame = params["frameId"].as_str().unwrap_or_default();
                let Some(bid) = self
                    .sessions
                    .owner(frame)
                    .or_else(|| self.sessions.most_recently_used())
                else {
                    return;
                };
                let name = safe_file_name(params["suggestedFilename"].as_str().unwrap_or_default());
                let destination = self
                    .config
                    .data_dir
                    .join(DOWNLOADS_DIR)
                    .join(guid)
                    .join(&name);
                self.out.event(
                    EVENT_DOWNLOAD,
                    &DownloadEvent {
                        bid,
                        name: name.clone(),
                        path: destination.display().to_string(),
                        size: -1,
                        done: false,
                    },
                );
                self.downloads.insert(
                    guid.to_owned(),
                    Download {
                        bid,
                        name,
                        destination,
                    },
                );
            }
            "Browser.downloadProgress" => {
                let Some(guid) = params["guid"].as_str() else {
                    return;
                };
                let Some(download) = self.downloads.get(guid) else {
                    return;
                };
                let size = params["receivedBytes"].as_f64().unwrap_or(0.0) as i64;
                match params["state"].as_str() {
                    Some("completed") => {
                        let file = params["filePath"]
                            .as_str()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| incoming(&self.config.data_dir).join(guid));
                        let destination = download.destination.clone();
                        let internal = self.internal.clone();
                        let guid = guid.to_owned();
                        tokio::task::spawn_blocking(move || {
                            let saved = destination
                                .parent()
                                .map_or(Ok(()), std::fs::create_dir_all)
                                .and_then(|()| std::fs::rename(&file, &destination));
                            let path = match saved {
                                Ok(()) => destination,
                                Err(error) => {
                                    log::warn!("[Mac browser] could not move a download: {error}");
                                    file
                                }
                            };
                            let _ = internal.send(Internal::DownloadSaved { guid, path, size });
                        });
                    }
                    // The protocol has no word for a download that never finished.
                    Some("canceled") => {
                        self.downloads.remove(guid);
                    }
                    Some(_) | None => {}
                }
            }
            _ => {}
        }
    }

    fn page_event(&mut self, devtools_session: &str, method: &str, params: Value) {
        if method == "Page.screencastFrame" {
            return self.frame(devtools_session, params);
        }
        let Some((bid, tab)) = self.sessions.tab_by_devtools_session(devtools_session) else {
            return;
        };
        let logged_at = match method {
            "Runtime.consoleAPICalled" | "Runtime.exceptionThrown" => params["timestamp"].as_f64(),
            "Log.entryAdded" => params["entry"]["timestamp"].as_f64(),
            _ => None,
        };
        if let Some(logged_at) = logged_at {
            if logged_at <= tab.console_replayed_up_to {
                return;
            }
            tab.console_newest = tab.console_newest.max(logged_at);
        }
        let target = tab.id.clone();
        let main_frame = |frame: &Value| frame.as_str() == Some(target.as_str());
        match method {
            "Page.frameStartedNavigating" => {
                let same_document = params["navigationType"].as_str() == Some("sameDocument");
                if main_frame(&params["frameId"]) && !same_document {
                    tab.console_replayed_up_to = tab.console_newest;
                    let local = is_local_url(params["url"].as_str().unwrap_or_default());
                    if local != tab.local {
                        tab.local = local;
                        self.set_local(devtools_session, local);
                    }
                }
            }
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                if !frame["parentId"].is_null() {
                    return;
                }
                let url = format!(
                    "{}{}",
                    frame["url"].as_str().unwrap_or_default(),
                    frame["urlFragment"].as_str().unwrap_or_default()
                );
                tab.url = url;
                tab.console_sent = 0;
                let local = is_local_url(&tab.url);
                if local != tab.local {
                    tab.local = local;
                    self.set_local(devtools_session, local);
                }
                if local {
                    self.add_pickers_script(devtools_session, &target);
                }
                self.pickers
                    .retain(|_, picker| picker.devtools_session != devtools_session);
                self.refresh_history(&target);
                self.send_state(bid);
            }
            "Page.navigatedWithinDocument" => {
                if main_frame(&params["frameId"]) {
                    tab.url = params["url"].as_str().unwrap_or_default().to_owned();
                    self.refresh_history(&target);
                    self.send_state(bid);
                }
            }
            "Page.frameStartedLoading" | "Page.frameStoppedLoading" => {
                if main_frame(&params["frameId"]) {
                    tab.loading = method == "Page.frameStartedLoading";
                    if !tab.loading {
                        self.refresh_history(&target);
                    }
                    self.send_state(bid);
                }
            }
            "Page.javascriptDialogOpening" => {
                let Some(kind) = params["type"].as_str().and_then(DialogKind::from_devtools) else {
                    return;
                };
                if let Some(session) = self.sessions.get_mut(bid) {
                    session.dialog = Some(target.clone());
                }
                self.out.event(
                    EVENT_DIALOG,
                    &DialogEvent {
                        bid,
                        tab: target,
                        kind,
                        message: truncate_chars(
                            params["message"].as_str().unwrap_or_default(),
                            10_000,
                        ),
                        default: params["defaultPrompt"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    },
                );
            }
            "Page.javascriptDialogClosed" => {
                if let Some(session) = self
                    .sessions
                    .get_mut(bid)
                    .filter(|session| session.dialog.as_ref() == Some(&target))
                {
                    session.dialog = None;
                }
                self.out
                    .event(EVENT_DIALOG_CLOSED, &DialogClosedEvent { bid, tab: target });
            }
            "Page.fileChooserOpened" => {
                let Some(backend_node) = params["backendNodeId"].as_i64() else {
                    return;
                };
                let multiple = params["mode"].as_str() == Some("selectMultiple");
                let Some(cdp) = self.cdp().cloned() else {
                    return;
                };
                let internal = self.internal.clone();
                let devtools_session = devtools_session.to_owned();
                tokio::spawn(async move {
                    let accept = cdp
                        .call(
                            Some(&devtools_session),
                            "DOM.describeNode",
                            json!({ "backendNodeId": backend_node }),
                        )
                        .await
                        .ok()
                        .and_then(|node| attribute(&node["node"]["attributes"], "accept"))
                        .unwrap_or_default();
                    let _ = internal.send(Internal::FileChooser {
                        bid,
                        tab: target,
                        multiple,
                        accept,
                        backend_node,
                    });
                });
            }
            "Runtime.bindingCalled" => {
                if params["name"].as_str() != Some(BINDING) || !tab.local {
                    return;
                }
                let (Some(payload), Some(context)) = (
                    params["payload"].as_str(),
                    params["executionContextId"].as_i64(),
                ) else {
                    return;
                };
                let Ok(picker) = serde_json::from_str::<PagePicker>(payload) else {
                    return;
                };
                let id = self.next_id();
                self.pickers.insert(
                    id.clone(),
                    PickerTarget {
                        bid,
                        devtools_session: devtools_session.to_owned(),
                        context,
                        page_id: picker.id.clone(),
                    },
                );
                self.out
                    .event(EVENT_PICKER, &PickerEvent::new(bid, target, id, picker));
            }
            "Runtime.consoleAPICalled" => {
                let text = params["args"]
                    .as_array()
                    .map(|args| args.iter().map(describe).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let frame = &params["stackTrace"]["callFrames"][0];
                self.console(
                    bid,
                    &target,
                    ConsoleLevel::from_console_call(params["type"].as_str().unwrap_or_default()),
                    text,
                    frame["url"].as_str(),
                    frame["lineNumber"].as_u64(),
                    None,
                );
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let text = details["exception"]["description"]
                    .as_str()
                    .or_else(|| details["text"].as_str())
                    .unwrap_or("Uncaught error")
                    .to_owned();
                self.console(
                    bid,
                    &target,
                    ConsoleLevel::Error,
                    text,
                    details["url"].as_str(),
                    details["lineNumber"].as_u64(),
                    None,
                );
            }
            "Log.entryAdded" => {
                let entry = &params["entry"];
                let text = entry["text"].as_str().unwrap_or_default().to_owned();
                let url = entry["url"].as_str();
                match (entry["source"].as_str(), entry["level"].as_str()) {
                    (Some("network"), _) => {
                        if url.is_some_and(is_local_url) {
                            let status = network_status(&text).unwrap_or(0);
                            self.console(
                                bid,
                                &target,
                                ConsoleLevel::Network,
                                text,
                                url,
                                None,
                                Some(status),
                            );
                        }
                    }
                    // Scripts' own errors and console calls come through `Runtime` already.
                    (Some("javascript" | "console-api"), _) => {}
                    (_, Some("error")) => {
                        let line = entry["lineNumber"].as_u64();
                        self.console(bid, &target, ConsoleLevel::Error, text, url, line, None);
                    }
                    (_, Some("warning")) => {
                        let line = entry["lineNumber"].as_u64();
                        self.console(bid, &target, ConsoleLevel::Warn, text, url, line, None);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn console(
        &mut self,
        bid: u32,
        target: &str,
        level: ConsoleLevel,
        text: String,
        source: Option<&str>,
        zero_based_line: Option<u64>,
        status: Option<u16>,
    ) {
        let Some((_, tab)) = self.sessions.tab_mut(target) else {
            return;
        };
        if !tab.local || tab.console_sent >= MAX_CONSOLE_MESSAGES {
            return;
        }
        tab.console_sent += 1;
        self.out.event(
            EVENT_CONSOLE,
            &ConsoleEvent {
                bid,
                tab: target.to_owned(),
                level,
                text: truncate_chars(&text, MAX_CONSOLE_CHARS),
                source: source
                    .filter(|source| !source.is_empty())
                    .map(str::to_owned),
                line: zero_based_line.and_then(|line| u32::try_from(line + 1).ok()),
                url: status.and(source).map(str::to_owned),
                status,
            },
        );
    }

    /// Turns on, or off, what pages of this computer get: their console, and the reports of the
    /// page scripts. Third-party pages such as sign-in pages never get it, since enabling
    /// `Runtime` is something they look for.
    fn set_local(&self, devtools_session: &str, on: bool) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let page = Some(devtools_session);
        if !on {
            cdp.send(page, "Runtime.disable", json!({}));
            cdp.send(page, "Log.disable", json!({}));
            return;
        }
        cdp.send(page, "Runtime.enable", json!({}));
        cdp.send(page, "Log.enable", json!({}));
        cdp.send(
            page,
            "Runtime.addBinding",
            json!({ "name": BINDING, "executionContextName": WORLD }),
        );
    }

    /// Runs the pickers script in an isolated world of the page that has just been committed in
    /// `frame`. Added to every new document instead, it would run before the binding it reports
    /// through is there, since the page of this computer loads faster than `Runtime` is turned
    /// on for it.
    fn add_pickers_script(&self, devtools_session: &str, frame: &str) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let devtools_session = devtools_session.to_owned();
        let frame = frame.to_owned();
        tokio::spawn(async move {
            let page = Some(devtools_session.as_str());
            let Ok(world) = cdp
                .call(
                    page,
                    "Page.createIsolatedWorld",
                    json!({ "frameId": frame, "worldName": WORLD }),
                )
                .await
            else {
                return;
            };
            cdp.send(
                page,
                "Runtime.evaluate",
                json!({ "expression": PICKERS_SCRIPT, "contextId": world["executionContextId"] }),
            );
        });
    }

    fn frame(&mut self, devtools_session: &str, params: Value) {
        let Some(screencast) = params["sessionId"].as_i64() else {
            return;
        };
        let watched = self
            .sessions
            .tab_by_devtools_session(devtools_session)
            .map(|(bid, tab)| (bid, tab.id.clone()))
            .filter(|(_, tab)| self.sessions.is_active(tab));
        let image = params["data"]
            .as_str()
            .and_then(|data| BASE64.decode(data).ok());
        let (Some((bid, _)), Some(image)) = (watched, image) else {
            if let Some(cdp) = self.cdp() {
                cdp.send(
                    Some(devtools_session),
                    "Page.screencastFrameAck",
                    json!({ "sessionId": screencast }),
                );
            }
            return;
        };
        let metadata = &params["metadata"];
        let side = |name: &str| {
            metadata[name]
                .as_f64()
                .unwrap_or(0.0)
                .round()
                .clamp(0.0, f64::from(u16::MAX)) as u16
        };
        let (css_width, css_height) = (side("deviceWidth"), side("deviceHeight"));
        let Some(session) = self.sessions.get_mut(bid) else {
            return;
        };
        let frame = session.frame_sent(devtools_session, screencast, Instant::now());
        let header = FrameHeader {
            bid,
            frame,
            css_width,
            css_height,
            format: ImageFormat::Jpeg,
        };
        // A picture that cannot be queued stays unacknowledged, which holds Chrome back until
        // the connection is back or the picture counts as lost.
        self.out.frame(encode_browser_frame(header, &image));
    }
}

/// Where a tab's history leads, from `Page.getNavigationHistory`.
#[derive(Debug, PartialEq, Eq)]
struct HistoryMoves {
    /// The entry going back leads to. The blank page every tab starts on is not one.
    back: Option<i64>,
    forward: Option<i64>,
    /// The title of the current page.
    title: Option<String>,
}

impl HistoryMoves {
    fn of(history: &Value) -> Self {
        let entries = history["entries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let current = history["currentIndex"].as_u64().unwrap_or(0) as usize;
        let id = |index: usize| entries.get(index).and_then(|entry| entry["id"].as_i64());
        let first = usize::from(
            entries
                .first()
                .is_some_and(|entry| entry["url"] == "about:blank"),
        );
        Self {
            back: current
                .checked_sub(1)
                .filter(|index| *index >= first)
                .and_then(id),
            forward: id(current + 1),
            title: entries
                .get(current)
                .and_then(|entry| entry["title"].as_str())
                .filter(|title| !title.is_empty())
                .map(str::to_owned),
        }
    }
}

async fn next_event(events: &mut Option<mpsc::UnboundedReceiver<CdpEvent>>) -> Option<CdpEvent> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

/// Where Chrome writes downloads while they arrive.
fn incoming(data_dir: &Path) -> PathBuf {
    data_dir.join(DOWNLOADS_DIR).join(".incoming")
}

/// Starts Chrome or finds it running, connects to it and sets up what every tab shares.
async fn launch(app: &Path, profile: &Path, downloads: &Path) -> Result<Started, RpcError> {
    let instance = chrome::start(app, profile)
        .await
        .map_err(RpcError::failed)?;
    let (cdp, events) = Cdp::connect(&instance.endpoint)
        .await
        .map_err(|error| RpcError::failed(format!("Could not connect to Chrome: {error}")))?;
    let incoming = downloads.join(".incoming");
    if let Err(error) = std::fs::create_dir_all(&incoming) {
        log::warn!(
            "[Mac browser] could not create {}: {error}",
            incoming.display()
        );
    }
    let setup = async {
        let targets = cdp.call(None, "Target.getTargets", json!({})).await?;
        let stale = targets["targetInfos"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|target| target["type"] == "page")
            .filter_map(|target| target["targetId"].as_str().map(str::to_owned))
            .collect::<HashSet<_>>();
        cdp.call(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
        )
        .await?;
        cdp.call(
            None,
            "Target.setAutoAttach",
            json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true,
                "filter": [{ "type": "page", "exclude": false }, { "exclude": true }],
            }),
        )
        .await?;
        cdp.call(
            None,
            "Browser.setDownloadBehavior",
            json!({
                "behavior": "allowAndName",
                "downloadPath": incoming.display().to_string(),
                "eventsEnabled": true,
            }),
        )
        .await?;
        let version = cdp.call(None, "Browser.getVersion", json!({})).await?;
        let user_agent = version["userAgent"].as_str().unwrap_or_default().to_owned();
        Ok::<_, String>((user_agent, stale))
    };
    let (user_agent, stale) = setup
        .await
        .map_err(|error| RpcError::failed(format!("Chrome did not accept the setup: {error}")))?;
    Ok(Started {
        cdp,
        events,
        instance,
        user_agent,
        stale,
    })
}

/// The value of attribute `name` in DevTools' flat list of attribute names and values.
fn attribute(attributes: &Value, name: &str) -> Option<String> {
    let attributes = attributes.as_array()?;
    attributes
        .chunks(2)
        .find(|pair| pair[0].as_str() == Some(name))
        .and_then(|pair| pair.get(1)?.as_str().map(str::to_owned))
}

/// A console argument as the console would print it.
fn describe(argument: &Value) -> String {
    match argument.get("value") {
        Some(Value::String(text)) => text.clone(),
        Some(value) if !value.is_null() => value.to_string(),
        Some(_) | None => argument["description"]
            .as_str()
            .or_else(|| argument["unserializableValue"].as_str())
            .or_else(|| argument["type"].as_str())
            .unwrap_or_default()
            .to_owned(),
    }
}

#[cfg(test)]
#[path = "browser_engine_tests.rs"]
mod tests;
