//! Previews: dev servers the owner's tools start in Hako, opened from the phone through byte
//! streams. This holds the registry and what decides a preview's status, the request files tools
//! write, the state file they read, and the check that a server answers. Opening tabs and typing
//! into them is the app's part.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::browser::PreviewAddress;

/// Under the data directory.
pub const PREVIEWS_DIR: &str = "previews";
pub const REQUESTS_DIR: &str = "requests";
pub const STATE_FILE: &str = "state.json";

pub const READY_POLL_INTERVAL: Duration = Duration::from_millis(500);
pub const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a server that was attached is given to answer.
pub const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);
pub const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
/// A running preview that has not answered for this long is stopped.
pub const HEALTH_GRACE: Duration = Duration::from_secs(15);
/// How often a preview that was asked to stop is checked.
pub const STOP_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a preview that was asked to stop may keep answering before it counts as running again.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a tab is given to be ready for the command: a new shell to start, or the old server
/// to end after Ctrl+C.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);

pub const MAX_ID_CHARS: usize = 40;
const RESERVED_IDS: [&str; 9] = [
    "www", "api", "app", "admin", "relay", "hako", "mail", "static", "assets",
];
const MAX_REJECTED: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    Starting,
    Running,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewPort {
    pub port: u16,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub repo: Option<String>,
    /// The main port.
    pub port: u16,
    /// Every port the browser may need, the main one first.
    pub ports: Vec<PreviewPort>,
    /// `None` for a server that was attached while already running.
    pub command: Option<String>,
    pub tab_id: Option<String>,
    pub session_id: Option<String>,
    pub status: PreviewStatus,
    pub message: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub started: u64,
    pub url_path: String,
}

/// What `preview.list` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewList {
    /// Newest first.
    pub previews: Vec<Preview>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewIdParams {
    pub id: String,
}

/// A preview as the state file keeps it, with what running it again takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPreview {
    #[serde(flatten)]
    pub preview: Preview,
    #[serde(default = "root_path")]
    pub ready_path: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A request file that could not be used, kept in the state file for the tool that wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub file: String,
    pub id: Option<String>,
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

/// The state file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewsState {
    pub previews: Vec<StoredPreview>,
    /// Milliseconds since the Unix epoch.
    pub updated: u64,
    #[serde(default)]
    pub rejected: Vec<Rejected>,
}

fn root_path() -> String {
    "/".to_owned()
}

/// A port given in a request, alone or with a label.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum PortRequest {
    Number(u16),
    Labelled {
        port: u16,
        #[serde(default)]
        label: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StartRequest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub directory: String,
    pub command: String,
    pub port: u16,
    /// The main port's label.
    #[serde(default)]
    pub label: Option<String>,
    /// The other ports.
    #[serde(default)]
    pub ports: Vec<PortRequest>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "root_path")]
    pub url_path: String,
    #[serde(default = "root_path")]
    pub ready_path: String,
    #[serde(default)]
    pub repo: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AttachRequest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub directory: String,
    pub port: u16,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub ports: Vec<PortRequest>,
    #[serde(default = "root_path")]
    pub url_path: String,
    #[serde(default)]
    pub repo: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PreviewRequest {
    Start(StartRequest),
    Attach(AttachRequest),
    Stop(PreviewIdParams),
    Restart(PreviewIdParams),
    Remove(PreviewIdParams),
}

/// Reads a request file. A request that cannot be read is returned as an error, with the id it
/// names when there is one.
pub fn parse_request(text: &str) -> Result<PreviewRequest, (Option<String>, String)> {
    serde_json::from_str(text).map_err(|error| {
        let id = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| value.get("id")?.as_str().map(str::to_owned));
        (id, format!("The request cannot be read: {error}"))
    })
}

/// Checks a preview id, which is the first label of the preview's address.
pub fn check_id(id: &str) -> Result<(), String> {
    let well_formed = !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if !well_formed {
        return Err(format!(
            "The preview id \"{id}\" must be 1 to {MAX_ID_CHARS} lowercase letters, digits and \
             hyphens, not starting or ending with a hyphen"
        ));
    }
    if id.contains("--") {
        return Err(format!(
            "The preview id \"{id}\" may not contain \"--\", which separates an id from a port \
             in a preview's address"
        ));
    }
    if RESERVED_IDS.contains(&id) {
        return Err(format!("\"{id}\" is reserved and cannot be a preview id"));
    }
    Ok(())
}

fn check_path(path: &str, what: &str) -> Result<(), String> {
    if path.starts_with('/') {
        Ok(())
    } else {
        Err(format!("{what} must start with \"/\", not \"{path}\""))
    }
}

fn check_env_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "\"{name}\" is not a valid environment variable name"
        ))
    }
}

fn check_common(id: &str, directory: &str, port: u16) -> Result<(), String> {
    check_id(id)?;
    if !Path::new(directory).is_absolute() {
        return Err(format!(
            "The directory must be an absolute path, not \"{directory}\""
        ));
    }
    if port == 0 {
        return Err("The port must be between 1 and 65535".to_owned());
    }
    Ok(())
}

/// The ports of a preview: the main one first, labelled `label` or else `name`, then the others
/// once each.
pub fn preview_ports(
    main: u16,
    label: Option<&str>,
    name: &str,
    others: &[PortRequest],
) -> Result<Vec<PreviewPort>, String> {
    let label_or = |label: Option<&str>, fallback: &str| {
        label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    };
    let mut ports = vec![PreviewPort {
        port: main,
        label: label_or(label, name),
    }];
    for other in others {
        let (port, label) = match other {
            PortRequest::Number(port) => (*port, None),
            PortRequest::Labelled { port, label } => (*port, label.as_deref()),
        };
        if port == 0 {
            return Err("A port must be between 1 and 65535".to_owned());
        }
        if ports.iter().all(|known| known.port != port) {
            ports.push(PreviewPort {
                port,
                label: label_or(label, &port.to_string()),
            });
        }
    }
    Ok(ports)
}

/// The shell language a command is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellSyntax {
    Posix,
    Fish,
}

fn quoted(text: &str, syntax: ShellSyntax) -> String {
    match syntax {
        ShellSyntax::Posix => format!("'{}'", text.replace('\'', r"'\''")),
        ShellSyntax::Fish => format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'")),
    }
}

/// The line run in a preview's tab: change to its directory, then run the command with `env`
/// exported for the command only.
pub fn command_line(
    directory: &str,
    command: &str,
    env: &BTreeMap<String, String>,
    syntax: ShellSyntax,
) -> String {
    let cd = format!("cd {}", quoted(directory, syntax));
    if env.is_empty() {
        return format!("{cd} && {command}");
    }
    match syntax {
        ShellSyntax::Posix => {
            let exports = env
                .iter()
                .map(|(name, value)| format!("{name}={}", quoted(value, syntax)))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{cd} && (export {exports}; {command})")
        }
        ShellSyntax::Fish => {
            let exports = env
                .iter()
                .map(|(name, value)| format!("set -lx {name} {}; ", quoted(value, syntax)))
                .collect::<String>();
            format!("{cd} && begin; {exports}{command}; end")
        }
    }
}

/// Whether an HTTP server answers on the port of this computer, with any status. Blocks for at
/// most about twice `wait`.
pub fn answers(port: u16, path: &str, wait: Duration) -> bool {
    let addresses = [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ];
    addresses.iter().any(|address| {
        ask(address, port, path, wait).unwrap_or_else(|error| {
            log::trace!("[Previews] {address}: {error}");
            false
        })
    })
}

fn ask(address: &SocketAddr, port: u16, path: &str, wait: Duration) -> io::Result<bool> {
    let mut stream = TcpStream::connect_timeout(address, wait)?;
    stream.set_read_timeout(Some(wait))?;
    stream.set_write_timeout(Some(wait))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nUser-Agent: Hako\r\n\
         Accept: */*\r\nConnection: close\r\n\r\n"
    )?;
    let mut start = [0; 5];
    let mut read = 0;
    let deadline = Instant::now() + wait;
    while read < start.len() && Instant::now() < deadline {
        match stream.read(&mut start[read..])? {
            0 => break,
            count => read += count,
        }
    }
    Ok(&start[..read] == b"HTTP/")
}

/// Writes the state file through a temporary file in the same directory, so that a reader
/// never sees half of it.
pub fn write_state(path: &Path, state: &PreviewsState) -> io::Result<()> {
    let directory = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".{STATE_FILE}.{}.tmp", std::process::id()));
    let text = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

/// Reads the state file, or an empty state when there is none or it cannot be read.
pub fn read_state(path: &Path) -> PreviewsState {
    fs::read(path)
        .ok()
        .and_then(|text| serde_json::from_slice(&text).ok())
        .unwrap_or_default()
}

/// A request file taken from the requests directory.
pub struct RequestFile {
    pub file: String,
    pub request: Result<PreviewRequest, (Option<String>, String)>,
}

/// Takes the request files that are complete, oldest first, and deletes them. Files whose name
/// starts with a dot or does not end in `.json` are still being written.
pub fn take_requests(directory: &Path) -> Vec<RequestFile> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') || !name.ends_with(".json") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
                (modified, name, entry.path())
            })
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|(_, file, path)| {
            let request = match fs::read_to_string(&path) {
                Ok(text) => parse_request(&text),
                Err(error) => Err((None, format!("The request cannot be read: {error}"))),
            };
            if let Err(error) = fs::remove_file(&path) {
                log::warn!("[Previews] could not remove the request {file}: {error}");
            }
            RequestFile { file, request }
        })
        .collect()
}

/// What a preview is doing, beyond its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The command waits for its tab to be ready for it.
    Launching {
        deadline: Instant,
    },
    /// Waits for the server to answer.
    Starting {
        deadline: Instant,
    },
    Running {
        last_answer: Instant,
    },
    /// Ctrl+C was sent.
    Stopping {
        deadline: Instant,
    },
    /// Stopped or failed. A preview that stopped answering by itself is running again once it
    /// answers.
    Idle {
        revive: bool,
    },
}

struct Entry {
    stored: StoredPreview,
    phase: Phase,
    /// Bumped with every change of phase, so that the answer of a check made before is ignored.
    generation: u64,
    next_check: Instant,
}

impl Entry {
    fn set(&mut self, status: PreviewStatus, message: Option<&str>, phase: Phase, now: Instant) {
        self.stored.preview.status = status;
        self.stored.preview.message = message.map(str::to_owned);
        self.phase = phase;
        self.generation += 1;
        self.next_check = now;
    }

    fn stopped(&mut self, message: &str, revive: bool, now: Instant) {
        self.set(
            PreviewStatus::Stopped,
            Some(message),
            Phase::Idle { revive },
            now,
        );
    }

    fn failed(&mut self, message: &str, now: Instant) {
        self.set(
            PreviewStatus::Failed,
            Some(message),
            Phase::Idle { revive: false },
            now,
        );
    }
}

/// A check of whether a preview's main port answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub id: String,
    pub generation: u64,
    pub port: u16,
    pub path: String,
    /// How long to wait for the answer.
    pub wait: Duration,
}

/// What the app has to do in a preview's tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabWork {
    /// Send Ctrl+C to the tab.
    Interrupt {
        tab_id: String,
    },
    Close {
        tab_id: String,
    },
}

/// The outcome of a request.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub work: Vec<TabWork>,
    pub changed: bool,
}

/// Every preview, newest first.
#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    rejected: Vec<Rejected>,
}

impl Registry {
    /// The previews kept from the last run, which come back stopped since their tabs are gone.
    pub fn restore(state: PreviewsState, now: Instant) -> Self {
        let entries = state
            .previews
            .into_iter()
            .filter(|stored| check_id(&stored.preview.id).is_ok())
            .map(|mut stored| {
                let preview = &mut stored.preview;
                preview.tab_id = None;
                preview.session_id = None;
                match preview.status {
                    PreviewStatus::Starting | PreviewStatus::Running => {
                        preview.status = PreviewStatus::Stopped;
                        preview.message = Some("Hako was restarted".to_owned());
                    }
                    PreviewStatus::Stopped | PreviewStatus::Failed => {}
                }
                Entry {
                    stored,
                    phase: Phase::Idle { revive: false },
                    generation: 0,
                    next_check: now,
                }
            })
            .collect();
        Self {
            entries,
            rejected: state.rejected,
        }
    }

    /// The state file's content.
    pub fn state(&self, updated: u64) -> PreviewsState {
        PreviewsState {
            previews: self
                .entries
                .iter()
                .map(|entry| entry.stored.clone())
                .collect(),
            updated,
            rejected: self.rejected.clone(),
        }
    }

    pub fn list(&self) -> Vec<Preview> {
        self.entries
            .iter()
            .map(|entry| entry.stored.preview.clone())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&StoredPreview> {
        self.entry(id).map(|entry| &entry.stored)
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.stored.preview.id == id)
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.stored.preview.id == id)
    }

    /// The ports streams may be opened to: those of the previews that are starting or running.
    pub fn open_ports(&self) -> HashMap<String, Vec<u16>> {
        self.entries
            .iter()
            .filter(|entry| match entry.stored.preview.status {
                PreviewStatus::Starting | PreviewStatus::Running => true,
                PreviewStatus::Stopped | PreviewStatus::Failed => false,
            })
            .map(|entry| {
                let preview = &entry.stored.preview;
                let ports = preview.ports.iter().map(|port| port.port).collect();
                (preview.id.clone(), ports)
            })
            .collect()
    }

    /// Where every preview's first page is, for the Mac browser, which opens stopped ones too.
    pub fn addresses(&self) -> HashMap<String, PreviewAddress> {
        self.entries
            .iter()
            .map(|entry| {
                let preview = &entry.stored.preview;
                let address = PreviewAddress {
                    port: preview.port,
                    url_path: preview.url_path.clone(),
                };
                (preview.id.clone(), address)
            })
            .collect()
    }

    /// The previews whose command waits for their tab, with the tab they have, if any.
    pub fn launching(&self) -> Vec<(String, Option<String>)> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.phase, Phase::Launching { .. }))
            .map(|entry| {
                let preview = &entry.stored.preview;
                (preview.id.clone(), preview.tab_id.clone())
            })
            .collect()
    }

    /// The tabs of the previews, by preview id.
    pub fn tabs(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .filter_map(|entry| {
                let preview = &entry.stored.preview;
                Some((preview.id.clone(), preview.tab_id.clone()?))
            })
            .collect()
    }

    /// Notes a request file that could not be used.
    pub fn reject(&mut self, file: String, id: Option<String>, message: String, at: u64) {
        log::info!("[Previews] request {file} refused: {message}");
        self.rejected.insert(
            0,
            Rejected {
                file,
                id,
                message,
                at,
            },
        );
        self.rejected.truncate(MAX_REJECTED);
    }

    /// Records a preview that could not be started, so that whoever asked learns why.
    pub fn fail_request(&mut self, id: &str, message: &str, started: u64, now: Instant) {
        let entry = self.upsert(
            StoredPreview {
                preview: Preview {
                    id: id.to_owned(),
                    name: id.to_owned(),
                    directory: String::new(),
                    repo: None,
                    port: 0,
                    ports: Vec::new(),
                    command: None,
                    tab_id: None,
                    session_id: None,
                    status: PreviewStatus::Failed,
                    message: None,
                    started,
                    url_path: root_path(),
                },
                ready_path: root_path(),
                env: BTreeMap::new(),
            },
            now,
        );
        entry.failed(message, now);
    }

    /// Puts `stored` first in place of a preview with the same id, keeping that one's tab.
    fn upsert(&mut self, mut stored: StoredPreview, now: Instant) -> &mut Entry {
        let id = stored.preview.id.clone();
        let old = self
            .entries
            .iter()
            .position(|entry| entry.stored.preview.id == id)
            .map(|index| self.entries.remove(index));
        let generation = old.as_ref().map_or(0, |old| old.generation + 1);
        if let Some(old) = &old {
            stored.preview.tab_id = old.stored.preview.tab_id.clone();
            stored.preview.session_id = old.stored.preview.session_id.clone();
        }
        self.entries.insert(
            0,
            Entry {
                stored,
                phase: Phase::Idle { revive: false },
                generation,
                next_check: now,
            },
        );
        &mut self.entries[0]
    }

    /// Starts a preview, or replaces the one with the same id. One whose command already runs
    /// for the same port is left running.
    pub fn start(&mut self, request: StartRequest, started: u64, now: Instant) -> Outcome {
        let checked = check_common(&request.id, &request.directory, request.port)
            .and_then(|()| check_path(&request.url_path, "url_path"))
            .and_then(|()| check_path(&request.ready_path, "ready_path"))
            .and_then(|()| request.env.keys().try_for_each(|name| check_env_name(name)))
            .and_then(|()| {
                (!request.command.trim().is_empty())
                    .then_some(())
                    .ok_or_else(|| "The command is empty".to_owned())
            });
        let name = display_name(request.name.as_deref(), &request.id);
        let ports = checked.and_then(|()| {
            preview_ports(
                request.port,
                request.label.as_deref(),
                &name,
                &request.ports,
            )
        });
        let ports = match ports {
            Ok(ports) => ports,
            Err(message) => {
                self.fail_request(&request.id, &message, started, now);
                return Outcome {
                    work: Vec::new(),
                    changed: true,
                };
            }
        };

        let old = self.entry(&request.id).map(|entry| {
            let preview = &entry.stored.preview;
            let active = match entry.phase {
                Phase::Launching { .. } | Phase::Starting { .. } | Phase::Running { .. } => true,
                Phase::Stopping { .. } | Phase::Idle { .. } => false,
            };
            let same = preview.port == request.port
                && preview.command.as_deref() == Some(request.command.as_str());
            (active, same, entry.phase, preview.clone())
        });
        let stored = StoredPreview {
            preview: Preview {
                id: request.id.clone(),
                name,
                directory: request.directory,
                repo: request.repo,
                port: request.port,
                ports,
                command: Some(request.command),
                tab_id: None,
                session_id: None,
                status: PreviewStatus::Starting,
                message: None,
                started,
                url_path: request.url_path,
            },
            ready_path: request.ready_path,
            env: request.env,
        };
        let mut work = Vec::new();
        let entry = self.upsert(stored, now);
        match old {
            Some((true, true, phase, preview)) => {
                // Already running what was asked for.
                entry.stored.preview.status = preview.status;
                entry.stored.preview.started = preview.started;
                entry.phase = phase;
            }
            Some((active, _, _, preview)) => {
                if active && let Some(tab_id) = preview.tab_id {
                    work.push(TabWork::Interrupt { tab_id });
                }
                entry.set(
                    PreviewStatus::Starting,
                    None,
                    Phase::Launching {
                        deadline: now + LAUNCH_TIMEOUT,
                    },
                    now,
                );
            }
            None => entry.set(
                PreviewStatus::Starting,
                None,
                Phase::Launching {
                    deadline: now + LAUNCH_TIMEOUT,
                },
                now,
            ),
        }
        Outcome {
            work,
            changed: true,
        }
    }

    /// Registers a server that already runs.
    pub fn attach(&mut self, request: AttachRequest, started: u64, now: Instant) -> Outcome {
        let name = display_name(request.name.as_deref(), &request.id);
        let ports = check_common(&request.id, &request.directory, request.port)
            .and_then(|()| check_path(&request.url_path, "url_path"))
            .and_then(|()| {
                preview_ports(
                    request.port,
                    request.label.as_deref(),
                    &name,
                    &request.ports,
                )
            });
        let ports = match ports {
            Ok(ports) => ports,
            Err(message) => {
                self.fail_request(&request.id, &message, started, now);
                return Outcome {
                    work: Vec::new(),
                    changed: true,
                };
            }
        };
        let mut work = Vec::new();
        if let Some(old) = self.entry(&request.id) {
            let preview = &old.stored.preview;
            let active = match old.phase {
                Phase::Launching { .. } | Phase::Starting { .. } | Phase::Running { .. } => true,
                Phase::Stopping { .. } | Phase::Idle { .. } => false,
            };
            if active
                && preview.command.is_some()
                && let Some(tab_id) = &preview.tab_id
            {
                work.push(TabWork::Interrupt {
                    tab_id: tab_id.clone(),
                });
            }
        }
        let stored = StoredPreview {
            preview: Preview {
                id: request.id,
                name,
                directory: request.directory,
                repo: request.repo,
                port: request.port,
                ports,
                command: None,
                tab_id: None,
                session_id: None,
                status: PreviewStatus::Starting,
                message: None,
                started,
                url_path: request.url_path,
            },
            ready_path: root_path(),
            env: BTreeMap::new(),
        };
        let entry = self.upsert(stored, now);
        entry.set(
            PreviewStatus::Starting,
            None,
            Phase::Starting {
                deadline: now + ATTACH_TIMEOUT,
            },
            now,
        );
        Outcome {
            work,
            changed: true,
        }
    }

    /// Stops a preview: Ctrl+C in its tab, or for one that was attached, no longer following it.
    pub fn stop(&mut self, id: &str, now: Instant) -> Result<Outcome, String> {
        let entry = self.entry_mut(id).ok_or_else(|| no_such_preview(id))?;
        let preview = &entry.stored.preview;
        let mut work = Vec::new();
        match (entry.phase, &preview.command, preview.tab_id.clone()) {
            (Phase::Idle { .. }, _, _) => {
                if let Phase::Idle { revive: true } = entry.phase {
                    entry.phase = Phase::Idle { revive: false };
                }
                return Ok(Outcome::default());
            }
            (Phase::Stopping { .. }, _, _) => return Ok(Outcome::default()),
            (Phase::Launching { .. }, _, _) => entry.stopped("Stopped", false, now),
            (Phase::Starting { .. } | Phase::Running { .. }, Some(_), Some(tab_id)) => {
                work.push(TabWork::Interrupt { tab_id });
                entry.phase = Phase::Stopping {
                    deadline: now + STOP_TIMEOUT,
                };
                entry.generation += 1;
                entry.next_check = now + STOP_POLL_INTERVAL;
                return Ok(Outcome {
                    work,
                    changed: false,
                });
            }
            (Phase::Starting { .. } | Phase::Running { .. }, Some(_), None) => {
                entry.stopped("Stopped", false, now);
            }
            (Phase::Starting { .. } | Phase::Running { .. }, None, _) => entry.stopped(
                "No longer followed by Hako; the server itself was started elsewhere",
                false,
                now,
            ),
        }
        Ok(Outcome {
            work,
            changed: true,
        })
    }

    /// Runs a preview's command again, after stopping it if it runs, in its tab or a new one. One
    /// that was attached is looked for again.
    pub fn restart(&mut self, id: &str, started: u64, now: Instant) -> Result<Outcome, String> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.stored.preview.id == id)
            .ok_or_else(|| no_such_preview(id))?;
        if self.entries[index].stored.preview.port == 0 {
            return Err(format!(
                "The preview \"{id}\" was never started; start it again with a full request"
            ));
        }
        let entry = self.entries.remove(index);
        self.entries.insert(0, entry);
        let entry = &mut self.entries[0];
        entry.stored.preview.started = started;
        let mut work = Vec::new();
        let active = match entry.phase {
            Phase::Launching { .. } | Phase::Starting { .. } | Phase::Running { .. } => true,
            Phase::Stopping { .. } | Phase::Idle { .. } => false,
        };
        let preview = &entry.stored.preview;
        if preview.command.is_none() {
            entry.set(
                PreviewStatus::Starting,
                None,
                Phase::Starting {
                    deadline: now + ATTACH_TIMEOUT,
                },
                now,
            );
        } else {
            if active && let Some(tab_id) = preview.tab_id.clone() {
                work.push(TabWork::Interrupt { tab_id });
            }
            entry.set(
                PreviewStatus::Starting,
                None,
                Phase::Launching {
                    deadline: now + LAUNCH_TIMEOUT,
                },
                now,
            );
        }
        Ok(Outcome {
            work,
            changed: true,
        })
    }

    /// Forgets a preview and closes its tab.
    pub fn remove(&mut self, id: &str) -> Result<Outcome, String> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.stored.preview.id == id)
            .ok_or_else(|| no_such_preview(id))?;
        let entry = self.entries.remove(index);
        let work = entry
            .stored
            .preview
            .tab_id
            .filter(|_| entry.stored.preview.command.is_some())
            .map(|tab_id| TabWork::Close { tab_id })
            .into_iter()
            .collect();
        Ok(Outcome {
            work,
            changed: true,
        })
    }

    /// Records the tab a preview's command runs in.
    pub fn set_tab(&mut self, id: &str, tab_id: String, session_id: String) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        let preview = &mut entry.stored.preview;
        let changed = preview.tab_id.as_ref() != Some(&tab_id)
            || preview.session_id.as_ref() != Some(&session_id);
        preview.tab_id = Some(tab_id);
        preview.session_id = Some(session_id);
        changed
    }

    /// Records that the command was run in the tab, which was ready for it.
    pub fn launched(&mut self, id: &str, now: Instant) {
        if let Some(entry) = self
            .entry_mut(id)
            .filter(|entry| matches!(entry.phase, Phase::Launching { .. }))
        {
            entry.phase = Phase::Starting {
                deadline: now + READY_TIMEOUT,
            };
            entry.generation += 1;
            entry.next_check = now + READY_POLL_INTERVAL;
        }
    }

    /// Fails a preview whose command could not be run.
    pub fn launch_failed(&mut self, id: &str, message: &str, now: Instant) -> bool {
        let Some(entry) = self
            .entry_mut(id)
            .filter(|entry| matches!(entry.phase, Phase::Launching { .. }))
        else {
            return false;
        };
        entry.failed(message, now);
        true
    }

    /// The preview whose tab is `tab_id`.
    pub fn with_tab(&self, tab_id: &str) -> Option<&Preview> {
        self.entries
            .iter()
            .map(|entry| &entry.stored.preview)
            .find(|preview| preview.tab_id.as_deref() == Some(tab_id))
    }

    /// Fails the previews whose tab has not been ready for their command in time.
    pub fn expire_launches(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if let Phase::Launching { deadline } = entry.phase
                && now >= deadline
            {
                let message = if entry.stored.preview.tab_id.is_some() {
                    "The tab was not ready for the command in time; the previous command may \
                     still be running"
                } else {
                    "No tab could be opened for the command"
                };
                entry.failed(message, now);
                changed = true;
            }
        }
        changed
    }

    /// Notes that the command running in a preview's tab has ended. Returns whether that changed
    /// the preview.
    pub fn command_ended(&mut self, id: &str, exit_status: i32, now: Instant) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        match entry.phase {
            Phase::Starting { .. } => {
                let message = format!(
                    "The command ended (exit status {exit_status}) before the server answered on \
                     port {}",
                    entry.stored.preview.port
                );
                entry.failed(&message, now);
                true
            }
            Phase::Running { .. } => {
                let message = format!("The command ended (exit status {exit_status})");
                entry.stopped(&message, false, now);
                true
            }
            Phase::Stopping { .. } => {
                entry.stopped("Stopped", false, now);
                true
            }
            Phase::Launching { .. } | Phase::Idle { .. } => false,
        }
    }

    /// Notes that a preview's tab is gone, which ends what ran in it.
    pub fn tab_closed(&mut self, id: &str, now: Instant) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        entry.stored.preview.tab_id = None;
        entry.stored.preview.session_id = None;
        match entry.phase {
            Phase::Launching { .. } | Phase::Starting { .. } | Phase::Running { .. } => {
                entry.stopped("The tab was closed", false, now);
            }
            Phase::Stopping { .. } => entry.stopped("Stopped", false, now),
            Phase::Idle { .. } => {}
        }
        true
    }

    /// The checks that are due, which are then due again after their interval.
    pub fn due_checks(&mut self, now: Instant) -> Vec<Check> {
        let mut checks = Vec::new();
        for entry in &mut self.entries {
            let (interval, wait) = match entry.phase {
                Phase::Starting { .. } => (READY_POLL_INTERVAL, Duration::from_secs(5)),
                Phase::Running { .. } | Phase::Idle { revive: true } => {
                    (HEALTH_INTERVAL, Duration::from_secs(3))
                }
                Phase::Stopping { .. } => (STOP_POLL_INTERVAL, Duration::from_secs(1)),
                Phase::Launching { .. } | Phase::Idle { revive: false } => continue,
            };
            if now < entry.next_check {
                continue;
            }
            entry.next_check = now + interval;
            let stored = &entry.stored;
            checks.push(Check {
                id: stored.preview.id.clone(),
                generation: entry.generation,
                port: stored.preview.port,
                path: stored.ready_path.clone(),
                wait,
            });
        }
        checks
    }

    /// Takes in the outcome of a check.
    pub fn checked(&mut self, check: &Check, answered: bool, now: Instant) -> Checked {
        let Some(entry) = self
            .entry_mut(&check.id)
            .filter(|entry| entry.generation == check.generation)
        else {
            return Checked::Unchanged;
        };
        match (entry.phase, answered) {
            (Phase::Starting { .. }, true) => {
                entry.set(
                    PreviewStatus::Running,
                    None,
                    Phase::Running { last_answer: now },
                    now,
                );
                entry.next_check = now + HEALTH_INTERVAL;
                Checked::Ready
            }
            (Phase::Starting { deadline }, false) if now >= deadline => {
                let preview = &entry.stored.preview;
                let message = if preview.command.is_some() {
                    format!(
                        "Nothing answered on port {} within {} seconds",
                        preview.port,
                        READY_TIMEOUT.as_secs()
                    )
                } else {
                    format!("Nothing answers on port {}", preview.port)
                };
                entry.failed(&message, now);
                Checked::Changed
            }
            (Phase::Running { .. }, true) => {
                entry.phase = Phase::Running { last_answer: now };
                Checked::Unchanged
            }
            (Phase::Running { last_answer }, false) if now - last_answer >= HEALTH_GRACE => {
                entry.stopped("The server stopped answering", true, now);
                entry.next_check = now + HEALTH_INTERVAL;
                Checked::Changed
            }
            (Phase::Idle { revive: true }, true) => {
                entry.set(
                    PreviewStatus::Running,
                    None,
                    Phase::Running { last_answer: now },
                    now,
                );
                entry.next_check = now + HEALTH_INTERVAL;
                Checked::Changed
            }
            (Phase::Stopping { .. }, false) => {
                entry.stopped("Stopped", false, now);
                Checked::Changed
            }
            (Phase::Stopping { deadline }, true) if now >= deadline => {
                entry.set(
                    PreviewStatus::Running,
                    Some("Ctrl+C did not stop the server"),
                    Phase::Running { last_answer: now },
                    now,
                );
                entry.next_check = now + HEALTH_INTERVAL;
                Checked::Changed
            }
            (Phase::Starting { .. } | Phase::Running { .. } | Phase::Stopping { .. }, _)
            | (Phase::Launching { .. } | Phase::Idle { .. }, _) => Checked::Unchanged,
        }
    }
}

/// What a check changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checked {
    Unchanged,
    Changed,
    /// A preview that was starting answers now.
    Ready,
}

fn display_name(name: Option<&str>, id: &str) -> String {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(id)
        .to_owned()
}

fn no_such_preview(id: &str) -> String {
    format!("No preview called \"{id}\"")
}

#[cfg(test)]
#[path = "previews_tests.rs"]
mod tests;
