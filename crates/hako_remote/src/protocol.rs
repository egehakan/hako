//! Frames, error codes and method payloads of version 1 of the remote protocol.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: u32 = 1;

pub const PING_INTERVAL: Duration = Duration::from_secs(25);
/// A connection that has delivered no frame of any kind for this long is dropped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Close code the relay sends to a connection that a newer one replaced.
pub const CLOSE_REPLACED: u16 = 4001;
/// How long to stay away after [`CLOSE_REPLACED`], so that two running apps do not keep taking
/// the relay's one agent slot from each other.
pub const REPLACED_RECONNECT_DELAY: Duration = Duration::from_secs(30);

pub const EVENT_WORKSPACE_CHANGED: &str = "workspace.changed";

/// A text frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Frame {
    #[serde(rename = "req")]
    Request {
        /// Opaque to the agent, which echoes it back unchanged.
        id: String,
        method: String,
        #[serde(default = "empty_object", deserialize_with = "object_or_empty")]
        params: Value,
    },
    #[serde(rename = "res")]
    Response(Response),
    #[serde(rename = "ntf")]
    Notification {
        method: String,
        #[serde(default = "empty_object", deserialize_with = "object_or_empty")]
        params: Value,
    },
    #[serde(rename = "evt")]
    Event {
        event: String,
        #[serde(default = "empty_object")]
        data: Value,
    },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "pong")]
    Pong,
}

impl Frame {
    pub fn event(event: &str, data: Value) -> Self {
        Self::Event {
            event: event.to_owned(),
            data,
        }
    }

    pub fn decode(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text)
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("a frame is always representable as JSON")
    }
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// Reads `params`, which a sender may leave out or send as null.
fn object_or_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Null => empty_object(),
        value => value,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn new(id: String, outcome: Result<Value, RpcError>) -> Self {
        match outcome {
            Ok(result) => Self {
                id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => Self {
                id,
                ok: false,
                result: None,
                error: Some(error),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AgentOffline,
    Timeout,
    BadRequest,
    NotFound,
    NotSupported,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn not_supported(method: &str) -> Self {
        Self::new(
            ErrorCode::NotSupported,
            format!("This version of Hako does not support {method}"),
        )
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Failed, message)
    }
}

/// The methods this version of the app answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    AppInfo,
    WorkspaceList,
    TabCreate,
    TabActivate,
    TabClose,
}

impl Method {
    /// The method called `name` on the wire, or `None` for one this version does not know.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "app.info" => Some(Self::AppInfo),
            "workspace.list" => Some(Self::WorkspaceList),
            "tab.create" => Some(Self::TabCreate),
            "tab.activate" => Some(Self::TabActivate),
            "tab.close" => Some(Self::TabClose),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    /// The computer's name as its owner set it, such as "Ege's MacBook Pro".
    pub name: String,
    pub version: String,
    /// The computer's network name.
    pub hostname: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub windows: Vec<Window>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    /// True for the frontmost window only.
    pub active: bool,
    pub folders: Vec<Folder>,
    /// In display order.
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub directory: String,
    /// `#RRGGBB`.
    pub color: Option<String>,
    /// `material:<key>` or `text:<text>`; `None` for the default icon.
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub active: bool,
    pub folder_id: Option<String>,
    pub panes: Vec<Pane>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    pub kind: PaneKind,
    pub title: String,
    pub focused: bool,
    /// Set for terminals only.
    pub session_id: Option<String>,
    /// Set for terminals only.
    pub cwd: Option<String>,
    pub agent: Option<AgentState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    Terminal,
    File,
    Other,
}

/// A coding agent running in a terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub name: String,
    pub status: AgentStatus,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Working,
    Waiting,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabCreateParams {
    #[serde(default)]
    pub window_id: Option<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabCreateResult {
    pub tab_id: String,
    pub session_id: String,
}

/// Params of the methods that act on one tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabParams {
    pub tab_id: String,
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
