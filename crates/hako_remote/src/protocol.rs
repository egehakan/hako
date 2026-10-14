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
pub const EVENT_TERM_SIZE: &str = "term.size";
pub const EVENT_TERM_CLOSED: &str = "term.closed";
pub const EVENT_GIT_CHANGED: &str = "git.changed";
pub const EVENT_NOTIFY: &str = "notify";

pub const NOTIFICATION_TERM_INPUT: &str = "term.input";

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
    NotADirectory,
    IsADirectory,
    PermissionDenied,
    Conflict,
    TooLarge,
    GitFailed,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    /// The limit that was passed, for some `too_large` errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            limit: None,
        }
    }

    pub fn too_large(message: impl Into<String>, limit: u64) -> Self {
        Self {
            limit: Some(limit),
            ..Self::new(ErrorCode::TooLarge, message)
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
    TabRename,
    FolderCreate,
    FolderRename,
    TermSubscribe,
    TermUnsubscribe,
    TermResize,
    TermReleaseSize,
    ThemeGet,
    FsRoots,
    FsList,
    FsRead,
    FsWrite,
    FsMkdir,
    FsStat,
    FsCreateFile,
    FsRename,
    FsCopy,
    FsMove,
    FsTrash,
    FsArchive,
    FsExtract,
    FsTransferInfo,
    FsDownload,
    FsUploadBegin,
    FsUploadChunk,
    FsUploadEnd,
    FsUploadCancel,
    GitRepos,
    GitStatus,
    GitDiff,
    GitCommit,
    GitPush,
    GitPull,
    GitDiscard,
    GitBranches,
    GitSwitch,
    GitLog,
    GitShow,
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
            "tab.rename" => Some(Self::TabRename),
            "folder.create" => Some(Self::FolderCreate),
            "folder.rename" => Some(Self::FolderRename),
            "term.subscribe" => Some(Self::TermSubscribe),
            "term.unsubscribe" => Some(Self::TermUnsubscribe),
            "term.resize" => Some(Self::TermResize),
            "term.release_size" => Some(Self::TermReleaseSize),
            "theme.get" => Some(Self::ThemeGet),
            "fs.roots" => Some(Self::FsRoots),
            "fs.list" => Some(Self::FsList),
            "fs.read" => Some(Self::FsRead),
            "fs.write" => Some(Self::FsWrite),
            "fs.mkdir" => Some(Self::FsMkdir),
            "fs.stat" => Some(Self::FsStat),
            "fs.create_file" => Some(Self::FsCreateFile),
            "fs.rename" => Some(Self::FsRename),
            "fs.copy" => Some(Self::FsCopy),
            "fs.move" => Some(Self::FsMove),
            "fs.trash" => Some(Self::FsTrash),
            "fs.archive" => Some(Self::FsArchive),
            "fs.extract" => Some(Self::FsExtract),
            "fs.transfer_info" => Some(Self::FsTransferInfo),
            "fs.download" => Some(Self::FsDownload),
            "fs.upload_begin" => Some(Self::FsUploadBegin),
            "fs.upload_chunk" => Some(Self::FsUploadChunk),
            "fs.upload_end" => Some(Self::FsUploadEnd),
            "fs.upload_cancel" => Some(Self::FsUploadCancel),
            "git.repos" => Some(Self::GitRepos),
            "git.status" => Some(Self::GitStatus),
            "git.diff" => Some(Self::GitDiff),
            "git.commit" => Some(Self::GitCommit),
            "git.push" => Some(Self::GitPush),
            "git.pull" => Some(Self::GitPull),
            "git.discard" => Some(Self::GitDiscard),
            "git.branches" => Some(Self::GitBranches),
            "git.switch" => Some(Self::GitSwitch),
            "git.log" => Some(Self::GitLog),
            "git.show" => Some(Self::GitShow),
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCreateParams {
    pub directory: String,
    #[serde(default)]
    pub window_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCreateResult {
    pub folder_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabRenameParams {
    pub tab_id: String,
    /// `None` goes back to the automatic title.
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRenameParams {
    pub folder_id: String,
    /// `None` goes back to the default name.
    #[serde(default)]
    pub name: Option<String>,
}

/// Params of the methods that act on one tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabParams {
    pub tab_id: String,
}

/// Params of the methods that act on one terminal session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionParams {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSubscribeResult {
    pub cols: usize,
    pub rows: usize,
    /// Offset of the first output byte the snapshot does not cover.
    pub seq: u64,
    /// Base64 of the bytes that bring a terminal emulator that has just been reset to the
    /// session's present state.
    pub snapshot: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermInputParams {
    pub session_id: String,
    /// Text to write to the PTY as it is.
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermResizeParams {
    pub session_id: String,
    pub cols: usize,
    pub rows: usize,
}

/// The size of a terminal, as the result of `term.resize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

/// Data of the `term.size` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSizeChanged {
    pub session_id: String,
    pub cols: usize,
    pub rows: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub terminal: TerminalColors,
    pub font_family: String,
    /// In points.
    pub font_size: f32,
}

/// Colours as `#RRGGBB`, or `#RRGGBBAA` for one that is not opaque.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalColors {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the bright eight.
    pub ansi: [String; 16],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRoots {
    pub home: String,
    /// Each path once.
    pub roots: Vec<FsRoot>,
}

/// A directory that browsing starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRoot {
    pub path: String,
    pub name: String,
    pub source: FsRootSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsRootSource {
    Folder,
    Terminal,
    Home,
}

/// Params of the methods that act on one path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsPathParams {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsList {
    pub entries: Vec<FsEntry>,
    /// Whether the directory holds more entries than these.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEntry {
    pub name: String,
    pub kind: FsEntryKind,
    /// What a symbolic link leads to in the end; `None` for one that leads nowhere, and for
    /// everything that is not a link.
    pub target: Option<FsLinkTarget>,
    /// In bytes, and of what a link leads to. Zero for a directory.
    pub size: u64,
    /// In milliseconds since the Unix epoch, and of what a link leads to.
    pub mtime: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsEntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsLinkTarget {
    Dir,
    File,
    Other,
}

/// The result of `fs.read`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsRead {
    Text {
        content: String,
        mtime: i64,
        size: u64,
    },
    Image {
        mime: String,
        /// The file's bytes in base64.
        data: String,
        mtime: i64,
        size: u64,
    },
    Binary {
        size: u64,
    },
    TooLarge {
        size: u64,
        /// The most bytes a file of this kind may have to be sent.
        limit: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsWriteParams {
    pub path: String,
    pub content: String,
    /// The `mtime` the file is expected to have, or `None` to write whatever it has. It has to
    /// be given either way.
    #[serde(deserialize_with = "Option::deserialize")]
    pub expected_mtime: Option<i64>,
}

/// The result of `fs.write`: what the file is like afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsWrite {
    pub mtime: i64,
    pub size: u64,
}

/// The result of `fs.stat`, which describes a link itself rather than what it points to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsStat {
    pub kind: FsEntryKind,
    /// In bytes; zero for a directory.
    pub size: u64,
    pub mtime: i64,
    /// What a link points to, as it is written in the link.
    pub target: Option<String>,
}

/// The result of `fs.create_file`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsCreateFile {
    pub mtime: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRenameParams {
    pub path: String,
    /// The new name, in the same directory.
    pub name: String,
}

/// A path a method answers with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsPath {
    pub path: String,
}

/// Params of `fs.copy` and `fs.move`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsCopyParams {
    pub sources: Vec<String>,
    /// The directory the items go into.
    pub destination: String,
    /// `None` means [`FsOnConflict::Fail`].
    #[serde(default)]
    pub on_conflict: Option<FsOnConflict>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsTrashParams {
    pub paths: Vec<String>,
}

/// What became of one item of a request that acts on several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsItemResult {
    pub source: String,
    /// Where the item is now; `None` when that is not known, as after an error.
    pub path: Option<String>,
    pub error: Option<RpcError>,
}

/// The result of `fs.copy`, `fs.move` and `fs.trash`: one entry per item, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsItemResults {
    pub results: Vec<FsItemResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsArchiveParams {
    pub path: String,
    /// Whether to leave out the directories that are only caches and dependencies; `None` means
    /// yes.
    #[serde(default)]
    pub skip_heavy: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsArchive {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// How many directories were left out.
    pub skipped: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsExtractParams {
    pub path: String,
    /// The directory the new directory is made in.
    pub destination: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsExtract {
    /// The new directory.
    pub path: String,
    /// How many files, directories and links were written.
    pub entries: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsTransferInfo {
    /// The longest file that is sent in either direction.
    pub max_bytes: u64,
    /// The most bytes in one piece, before base64.
    pub chunk_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDownloadParams {
    pub path: String,
    pub offset: u64,
    pub length: u64,
    /// The size the file had at the first piece, or `None` for the first piece. It has to be
    /// given either way, and is `None` exactly when `expected_mtime` is.
    #[serde(deserialize_with = "Option::deserialize")]
    pub expected_size: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub expected_mtime: Option<i64>,
}

/// One piece of a file, and what the file was like when it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDownload {
    /// The piece's bytes in base64.
    pub data: String,
    pub size: u64,
    pub mtime: i64,
}

/// Where an uploaded file goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsUploadDestination {
    Dir {
        path: String,
    },
    /// The app's own area for files that are handed to a program by their path.
    Uploads,
}

/// What happens when a directory already holds something of the uploaded file's name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsOnConflict {
    #[default]
    Fail,
    Replace,
    /// Keep both, giving the new file the first free numbered name.
    Rename,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsUploadBeginParams {
    pub destination: FsUploadDestination,
    pub name: String,
    pub size: u64,
    /// `None` means [`FsOnConflict::Fail`].
    #[serde(default)]
    pub on_conflict: Option<FsOnConflict>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsUploadBegin {
    pub upload_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsUploadChunkParams {
    pub upload_id: String,
    pub offset: u64,
    /// The piece's bytes in base64.
    pub data: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsUploadChunk {
    /// How many bytes of the file have arrived.
    pub received: u64,
}

/// Params of the upload methods that name only the upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsUploadIdParams {
    pub upload_id: String,
}

/// The result of `fs.upload_end`: the file as it now is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsUploadEnd {
    /// Absolute, and for a directory the destination's path joined with `name`.
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime: i64,
}

/// A working tree, as the source control methods describe it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepo {
    /// The root of the working tree, which is what names the repository.
    pub path: String,
    pub name: String,
    /// The directory of the folder the working tree belongs to.
    pub root: String,
    pub is_worktree: bool,
    /// Set for a linked worktree of a repository that has a working tree of its own.
    pub main_path: Option<String>,
    /// `None` while `HEAD` is detached.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    /// How many files `git.status` lists, before its limit.
    pub changed: usize,
    pub operation: Option<GitOperation>,
}

/// What is unfinished in a working tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitOperation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

/// A file that differs from `HEAD`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFile {
    /// Relative to the root of the working tree.
    pub path: String,
    /// Where a renamed file was.
    pub old_path: Option<String>,
    pub status: GitFileStatus,
    /// `None` for a binary file, and for one whose lines were not counted.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// Params of the methods that act on a whole repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepoParams {
    pub repo: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiffParams {
    pub repo: String,
    pub path: String,
    #[serde(default)]
    pub old_path: Option<String>,
    /// The commit whose change to the file is asked for; `None` for the working tree against
    /// `HEAD`.
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitParams {
    pub repo: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiscardParams {
    pub repo: String,
    /// The one file to put back; `None` for every change in the repository.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub old_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepos {
    pub repos: Vec<GitRepo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    pub repo: GitRepo,
    pub files: Vec<GitFile>,
    /// Whether more files differ than these.
    pub truncated: bool,
}

/// The result of `git.diff`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitDiff {
    Text {
        diff: String,
    },
    Binary {
        /// In bytes, in `HEAD`; `None` when the file is not there.
        old_size: Option<u64>,
        /// In bytes, in the working tree; `None` when the file is not there.
        new_size: Option<u64>,
    },
    TooLarge {
        /// The most bytes a diff may have to be sent.
        limit: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommit {
    /// The full id of the new commit.
    pub commit: String,
    pub output: String,
    pub repo: GitRepo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPush {
    pub output: String,
    /// Whether the branch was given an upstream by this push.
    pub published: bool,
    pub repo: GitRepo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPull {
    pub output: String,
    pub repo: GitRepo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiscard {
    pub repo: GitRepo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSwitchParams {
    pub repo: String,
    pub branch: String,
    pub kind: GitSwitchKind,
}

/// What `git.switch` is to do with the branch it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSwitchKind {
    /// Check out a local branch.
    Local,
    /// Make a local branch that tracks the remote-tracking branch named, and check it out.
    Remote,
    /// Make a branch at `HEAD` and check it out.
    New,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitLogParams {
    pub repo: String,
    /// How many commits to answer with; `None` for the default.
    #[serde(default)]
    pub limit: Option<u64>,
    /// How many of the newest commits to pass over.
    #[serde(default)]
    pub skip: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitShowParams {
    pub repo: String,
    /// A full or abbreviated commit id.
    pub commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranch {
    /// Short: `main` for a local branch, `origin/main` for a remote-tracking one.
    pub name: String,
    pub kind: GitBranchKind,
    /// The full id of the commit the branch points at.
    pub tip: String,
    /// When the tip was committed, in milliseconds since the epoch.
    pub time: i64,
    /// The first line of the tip's message.
    pub subject: String,
    /// The short name of what a local branch tracks.
    pub upstream: Option<String>,
    /// `None` without an upstream.
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// The other working tree that has a local branch checked out.
    pub worktree: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitBranchKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranches {
    /// The branch that is checked out; `None` while `HEAD` is detached.
    pub current: Option<String>,
    pub detached: bool,
    /// `None` before the first commit.
    pub head: Option<String>,
    /// Local branches before remote-tracking ones, each kind with its newest tip first.
    pub branches: Vec<GitBranch>,
    /// Whether the repository has more branches than these.
    pub truncated: bool,
}

/// The result of `git.switch`: where `HEAD` is afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSwitch {
    pub current: Option<String>,
    pub head: Option<String>,
}

/// A commit in the history of a branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitLogCommit {
    pub id: String,
    /// Full ids, the first parent first.
    pub parents: Vec<String>,
    /// The first line of the message.
    pub subject: String,
    /// The author's name.
    pub author: String,
    /// When it was written, in milliseconds since the epoch.
    pub time: i64,
    /// The branches and tags that point at it.
    pub refs: Vec<GitRef>,
    pub side: GitSide,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRef {
    pub name: String,
    pub kind: GitRefKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitRefKind {
    /// The branch that is checked out, or `HEAD` itself while it is detached.
    Head,
    Local,
    Remote,
    Tag,
}

/// Which of `HEAD` and the upstream of its branch a commit can be reached from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSide {
    Both,
    /// Only from `HEAD`: not pushed yet.
    Local,
    /// Only from the upstream: not pulled yet.
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitLog {
    /// Newest first, and a commit always before its parents.
    pub commits: Vec<GitLogCommit>,
    /// Whether older commits follow these.
    pub has_more: bool,
    pub head: Option<String>,
    /// The commit the upstream of the current branch is at.
    pub upstream: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitShow {
    pub commit: GitLogCommit,
    /// The message after its first line.
    pub body: String,
    /// What the commit changed against its first parent.
    pub files: Vec<GitFile>,
    /// Whether it changed more files than these.
    pub truncated: bool,
}

/// Data of the `git.changed` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChanged {
    /// The repository that may have changed; `None` when any of them may have.
    pub repo: Option<String>,
}

/// Data of the `notify` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notify {
    pub kind: NotifyKind,
    /// One line of at most [`NOTIFY_TITLE_MAX_CHARS`] characters.
    pub title: String,
    /// At most [`NOTIFY_BODY_MAX_CHARS`] characters, lines separated by `\n`.
    pub body: String,
    pub session_id: String,
    pub tab_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    AgentWaiting,
    AgentDone,
    CommandDone,
}

pub const NOTIFY_TITLE_MAX_CHARS: usize = 100;
pub const NOTIFY_BODY_MAX_CHARS: usize = 300;

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
