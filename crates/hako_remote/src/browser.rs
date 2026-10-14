//! The Mac browser: a Google Chrome on this computer that a remote client watches as a stream of
//! pictures and drives with input, so that an app under development runs against `localhost`
//! exactly as it does for its owner at the Mac. This module holds the wire types of the
//! `browser.*` methods and events and the binary frame that carries a picture.
//!
//! The binary frame:
//!
//! ```text
//! byte 0       0x03
//! bytes 1-4    bid, unsigned 32-bit big-endian
//! bytes 5-8    frame number, unsigned 32-bit big-endian
//! bytes 9-10   CSS width, unsigned 16-bit big-endian
//! bytes 11-12  CSS height, unsigned 16-bit big-endian
//! byte 13      image format: 1 JPEG, 2 PNG
//! rest         the image
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::browser_input::InputEvent;
use crate::protocol::RpcError;

pub const BROWSER_FRAME_TYPE: u8 = 0x03;
pub const BROWSER_FRAME_HEADER_BYTES: usize = 14;

pub const METHOD_PREFIX: &str = "browser.";

pub const EVENT_STATE: &str = "browser.state";
pub const EVENT_DIALOG: &str = "browser.dialog";
pub const EVENT_DIALOG_CLOSED: &str = "browser.dialog_closed";
pub const EVENT_PICKER: &str = "browser.picker";
pub const EVENT_FILE_CHOOSER: &str = "browser.file_chooser";
pub const EVENT_DOWNLOAD: &str = "browser.download";
pub const EVENT_CONSOLE: &str = "browser.console";
pub const EVENT_CLOSED: &str = "browser.closed";

pub const MAX_VIEWPORT_SIDE: u32 = 4096;
pub const MAX_SCALE: f64 = 3.0;
/// Console messages sent per page load.
pub const MAX_CONSOLE_MESSAGES: usize = 200;
pub const MAX_CONSOLE_CHARS: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ImageFormat {
    Jpeg = 1,
    Png = 2,
}

/// The header of a picture frame, which the image follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub bid: u32,
    pub frame: u32,
    pub css_width: u16,
    pub css_height: u16,
    pub format: ImageFormat,
}

pub fn encode_browser_frame(header: FrameHeader, image: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(BROWSER_FRAME_HEADER_BYTES + image.len());
    frame.push(BROWSER_FRAME_TYPE);
    frame.extend_from_slice(&header.bid.to_be_bytes());
    frame.extend_from_slice(&header.frame.to_be_bytes());
    frame.extend_from_slice(&header.css_width.to_be_bytes());
    frame.extend_from_slice(&header.css_height.to_be_bytes());
    frame.push(header.format as u8);
    frame.extend_from_slice(image);
    frame
}

/// The `browser.*` methods, requests and notifications alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserMethod {
    Open,
    Close,
    Navigate,
    Back,
    Forward,
    Reload,
    StopLoading,
    Viewport,
    SwitchTab,
    CloseTab,
    DialogReply,
    PickerReply,
    FileChooserReply,
    Copy,
    /// A notification.
    Ack,
    /// A notification.
    Input,
}

impl BrowserMethod {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "browser.open" => Some(Self::Open),
            "browser.close" => Some(Self::Close),
            "browser.navigate" => Some(Self::Navigate),
            "browser.back" => Some(Self::Back),
            "browser.forward" => Some(Self::Forward),
            "browser.reload" => Some(Self::Reload),
            "browser.stop_loading" => Some(Self::StopLoading),
            "browser.viewport" => Some(Self::Viewport),
            "browser.switch_tab" => Some(Self::SwitchTab),
            "browser.close_tab" => Some(Self::CloseTab),
            "browser.dialog_reply" => Some(Self::DialogReply),
            "browser.picker_reply" => Some(Self::PickerReply),
            "browser.file_chooser_reply" => Some(Self::FileChooserReply),
            "browser.copy" => Some(Self::Copy),
            "browser.ack" => Some(Self::Ack),
            "browser.input" => Some(Self::Input),
            _ => None,
        }
    }
}

/// What the page is shown as, in CSS pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    /// The device pixel ratio.
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub mobile: bool,
    #[serde(default)]
    pub touch: bool,
    /// The browser's own when left out.
    #[serde(default)]
    pub user_agent: Option<String>,
}

fn one() -> f64 {
    1.0
}

impl Viewport {
    pub fn validate(&self) -> Result<(), RpcError> {
        let sides = 1..=MAX_VIEWPORT_SIDE;
        if !sides.contains(&self.width) || !sides.contains(&self.height) {
            return Err(RpcError::bad_request(format!(
                "The viewport must be between 1 and {MAX_VIEWPORT_SIDE} pixels on each side"
            )));
        }
        if !(1.0..=MAX_SCALE).contains(&self.scale) {
            return Err(RpcError::bad_request(
                "The viewport scale must be between 1 and 3",
            ));
        }
        Ok(())
    }

    pub fn landscape(&self) -> bool {
        self.width > self.height
    }

    /// The largest picture to ask for, in device pixels.
    pub fn image_size(&self) -> (u32, u32) {
        (
            (f64::from(self.width) * self.scale).round() as u32,
            (f64::from(self.height) * self.scale).round() as u32,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OpenParams {
    #[serde(default)]
    pub preview_id: Option<String>,
    /// An address, or with `preview_id` a path of the preview or an address.
    #[serde(default)]
    pub url: Option<String>,
    pub viewport: Viewport,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpenResult {
    pub bid: u32,
    pub tabs: Vec<TabInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct SessionParams {
    pub bid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NavigateParams {
    pub bid: u32,
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct ReloadParams {
    pub bid: u32,
    /// Bypasses the cache.
    #[serde(default)]
    pub hard: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ViewportParams {
    pub bid: u32,
    pub viewport: Viewport,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TabParams {
    pub bid: u32,
    pub tab: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DialogReplyParams {
    pub bid: u32,
    pub accept: bool,
    /// What a prompt is answered with.
    #[serde(default)]
    pub text: Option<String>,
    /// The tab showing the dialog; the active one when left out.
    #[serde(default)]
    pub tab: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PickerReplyParams {
    pub bid: u32,
    pub id: String,
    /// The values chosen: those of the selected options of a `<select>`, or the one new value
    /// of an input.
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FileChooserReplyParams {
    pub bid: u32,
    pub id: String,
    /// Absolute paths on this computer; none cancels.
    pub paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct AckParams {
    pub bid: u32,
    pub frame: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct InputParams {
    pub bid: u32,
    pub events: Vec<InputEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CopyResult {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
}

/// Data of `browser.state`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BrowserState {
    pub bid: u32,
    /// The active tab.
    pub tab: Option<String>,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub tabs: Vec<TabInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
    Beforeunload,
}

impl DialogKind {
    /// The kind of a dialog as the DevTools protocol names it.
    pub fn from_devtools(kind: &str) -> Option<Self> {
        match kind {
            "alert" => Some(Self::Alert),
            "confirm" => Some(Self::Confirm),
            "prompt" => Some(Self::Prompt),
            "beforeunload" => Some(Self::Beforeunload),
            _ => None,
        }
    }
}

/// Data of `browser.dialog`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DialogEvent {
    pub bid: u32,
    pub tab: String,
    pub kind: DialogKind,
    pub message: String,
    /// The text a prompt starts with.
    pub default: String,
}

/// Data of `browser.dialog_closed`: the dialog went away without a reply from this client,
/// answered elsewhere or by the page navigating away.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DialogClosedEvent {
    pub bid: u32,
    pub tab: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickerKind {
    #[serde(rename = "select")]
    Select,
    #[serde(rename = "date")]
    Date,
    #[serde(rename = "time")]
    Time,
    #[serde(rename = "datetime-local")]
    DatetimeLocal,
    #[serde(rename = "month")]
    Month,
    #[serde(rename = "week")]
    Week,
    #[serde(rename = "color")]
    Color,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerOption {
    pub label: String,
    pub value: String,
    pub selected: bool,
    pub disabled: bool,
    /// The label of the `<optgroup>` it is in.
    #[serde(default)]
    pub group: Option<String>,
}

/// What the script in a page reports about a picker it kept from opening.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PagePicker {
    pub kind: PickerKind,
    /// Unique within the page.
    pub id: String,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub options: Option<Vec<PickerOption>>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
}

/// Data of `browser.picker`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PickerEvent {
    pub bid: u32,
    pub tab: String,
    pub kind: PickerKind,
    pub id: String,
    pub multiple: bool,
    /// For `select`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<PickerOption>>,
    /// For the inputs: the current value, in the input's own format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

impl PickerEvent {
    pub fn new(bid: u32, tab: String, id: String, picker: PagePicker) -> Self {
        Self {
            bid,
            tab,
            kind: picker.kind,
            id,
            multiple: picker.multiple,
            options: picker.options,
            value: picker.value,
            min: picker.min,
            max: picker.max,
            step: picker.step,
        }
    }
}

/// Data of `browser.file_chooser`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileChooserEvent {
    pub bid: u32,
    pub tab: String,
    pub id: String,
    pub multiple: bool,
    /// The input's `accept` attribute; empty without one.
    pub accept: String,
}

/// Data of `browser.download`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DownloadEvent {
    pub bid: u32,
    pub name: String,
    /// Where the file is on this computer once it is done.
    pub path: String,
    /// In bytes; -1 while not known.
    pub size: i64,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
    /// A request that failed or was answered with an error status.
    Network,
}

impl ConsoleLevel {
    /// The level of a `console` call as the DevTools protocol names it.
    pub fn from_console_call(kind: &str) -> Self {
        match kind {
            "error" | "assert" => Self::Error,
            "warning" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            _ => Self::Log,
        }
    }
}

/// Data of `browser.console`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConsoleEvent {
    pub bid: u32,
    pub tab: String,
    pub level: ConsoleLevel,
    pub text: String,
    /// The script, or for `network` the address requested.
    pub source: Option<String>,
    /// One-based.
    pub line: Option<u32>,
    /// For `network`, the address requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// For `network`, the HTTP status; 0 when no answer came.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosedReason {
    /// Its last tab closed.
    TabsClosed,
    /// No frame was acknowledged for a long time.
    Unacknowledged,
    /// Chrome quit or could no longer be reached.
    BrowserExited,
}

impl ClosedReason {
    /// The sentence clients show.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::TabsClosed => "Its last tab closed",
            Self::Unacknowledged => "Nothing was shown for 5 minutes",
            Self::BrowserExited => "Chrome quit on the Mac",
        }
    }
}

/// Data of `browser.closed`: the session ended without the client asking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClosedEvent {
    pub bid: u32,
    pub reason: String,
}

impl ClosedEvent {
    pub fn new(bid: u32, reason: ClosedReason) -> Self {
        Self {
            bid,
            reason: reason.sentence().to_owned(),
        }
    }
}

/// Where a preview's pages are, by preview id. Shared between the app, which knows the previews,
/// and the browser, which opens them.
#[derive(Clone, Default)]
pub struct PreviewAddresses(Arc<RwLock<HashMap<String, PreviewAddress>>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewAddress {
    pub port: u16,
    pub url_path: String,
}

impl PreviewAddresses {
    pub fn set(&self, addresses: HashMap<String, PreviewAddress>) {
        if let Ok(mut current) = self.0.write() {
            *current = addresses;
        }
    }

    pub fn get(&self, preview: &str) -> Option<PreviewAddress> {
        self.0.read().ok()?.get(preview).cloned()
    }
}

/// Whether `host` names this computer, as a dev server's address does.
pub fn is_local_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Host::Ipv4(address) => address.is_loopback() || address.is_unspecified(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

/// Whether `url` is a page of a dev server on this computer, where the console is read and the
/// page scripts run.
pub fn is_local_url(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some_and(|host| is_local_host(&host))
    })
}

/// The address to open for what the owner typed: `http` for this computer and `https` elsewhere
/// when no scheme is given. Only web pages and `about:blank` are opened.
pub fn resolve_url(input: &str) -> Result<String, RpcError> {
    let input = input.trim();
    let invalid = || RpcError::bad_request(format!("Not an address that can be opened: {input}"));
    if input.eq_ignore_ascii_case("about:blank") {
        return Ok("about:blank".to_owned());
    }
    let parsed = if input.contains("://") {
        Url::parse(input).map_err(|_| invalid())?
    } else {
        let guess = Url::parse(&format!("http://{input}")).map_err(|_| invalid())?;
        if guess.host().is_some_and(|host| is_local_host(&host)) {
            guess
        } else {
            Url::parse(&format!("https://{input}")).map_err(|_| invalid())?
        }
    };
    let has_host = parsed.host_str().is_some_and(|host| !host.is_empty());
    if matches!(parsed.scheme(), "http" | "https") && has_host {
        Ok(parsed.into())
    } else {
        Err(invalid())
    }
}

/// The address to open for a preview: its first page, or `url` when that is a path of the
/// preview or an address of its own.
pub fn preview_url(address: &PreviewAddress, url: Option<&str>) -> Result<String, RpcError> {
    let path = match url.map(str::trim) {
        None | Some("") => address.url_path.as_str(),
        Some(path) if path.starts_with('/') => path,
        Some(url) => return resolve_url(url),
    };
    resolve_url(&format!("http://localhost:{}{path}", address.port))
}

/// Cuts `text` to at most `max` characters.
pub fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

/// The HTTP status in Chrome's message about a request that failed, such as "the server
/// responded with a status of 404 (Not Found)".
pub fn network_status(text: &str) -> Option<u16> {
    let (_, rest) = text.split_once("status of ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A file name that is safe to create in a directory: no separators, not hidden, never empty.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        "download".to_owned()
    } else {
        truncate_chars(cleaned, 200)
    }
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
