//! Starting and stopping the Google Chrome that the Mac browser drives: a separate instance with
//! a profile of its own and a fixed debugging port on the loopback interface, never the owner's
//! own Chrome.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

/// The profile directory, inside the app's data directory. Sign-ins made in the browser are
/// kept there.
pub const PROFILE_DIR: &str = "preview-browser";
/// Where downloads go, inside the app's data directory.
pub const DOWNLOADS_DIR: &str = "preview-downloads";
/// Downloads are kept this long.
pub const DOWNLOADS_KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The debugging port is the first free one from here. A fixed port, unlike port 0, does not
/// make Chrome set `navigator.webdriver`, which sign-in pages such as Google's refuse.
const FIRST_PORT: u16 = 39222;
const PORTS_TRIED: u16 = 100;
const START_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

/// The app bundles that are looked for, in order, under `/Applications` and then
/// `~/Applications`.
const APP_NAMES: [&str; 2] = ["Google Chrome.app", "Chromium.app"];

/// The Chrome to start: `/Applications` first, then `~/Applications`, Google Chrome before
/// Chromium.
pub fn find_app(home: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    APP_NAMES.iter().find_map(|name| {
        [PathBuf::from("/Applications"), home.join("Applications")]
            .into_iter()
            .map(|folder| folder.join(name))
            .find(|app| exists(app))
    })
}

/// The arguments Chrome is started with. No automation flags: they change how sign-in pages
/// treat the browser.
pub fn arguments(profile: &Path, port: u16) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile.display()),
        format!("--remote-debugging-port={port}"),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        // Windows are opened later through the protocol, in the background and off the screen;
        // a window at start would come to the front.
        "--no-startup-window".to_owned(),
        // A window that is covered or off the screen keeps drawing, which the pictures need.
        "--disable-backgrounding-occluded-windows".to_owned(),
        "--disable-renderer-backgrounding".to_owned(),
        "--disable-background-timer-throttling".to_owned(),
        "--hide-crash-restore-bubble".to_owned(),
    ]
}

/// A running browser process of `profile`, from the output of `ps -axo pid=,args=`: its pid
/// and its debugging port. Helper processes, which carry `--type=`, are passed over.
pub fn find_process(ps_output: &str, profile: &Path) -> Option<(u32, Option<u16>)> {
    let profile_argument = format!("--user-data-dir={}", profile.display());
    ps_output.lines().find_map(|line| {
        let line = line.trim_start();
        let (pid, arguments) = line.split_once(char::is_whitespace)?;
        let pid = pid.parse().ok()?;
        if arguments.contains(" --type=") {
            return None;
        }
        let rest = arguments.split(profile_argument.as_str()).nth(1)?;
        // The profile argument must end where the path does.
        if !(rest.is_empty() || rest.starts_with(' ')) {
            return None;
        }
        let port = arguments
            .split(' ')
            .find_map(|argument| argument.strip_prefix("--remote-debugging-port="))
            .and_then(|port| port.parse().ok());
        Some((pid, port))
    })
}

/// A browser this app started, or found running with its profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub pid: u32,
    pub port: u16,
    /// The browser's own DevTools endpoint.
    pub endpoint: String,
}

/// Finds the browser of `profile` running, or starts it with `app`, and waits until its
/// debugging endpoint answers. Blocking work runs off the async threads.
pub async fn start(app: &Path, profile: &Path) -> Result<Instance, String> {
    std::fs::create_dir_all(profile)
        .map_err(|error| format!("Could not create {}: {error}", profile.display()))?;
    if let Some(instance) = running(profile).await {
        return Ok(instance);
    }
    let port = free_port().ok_or("No free port was found for the browser")?;
    let mut command = Command::new("/usr/bin/open");
    // In the background, as a new instance; not hidden, since a hidden app's windows do not draw.
    command
        .args(["-g", "-n", "-a"])
        .arg(app)
        .arg("--args")
        .args(arguments(profile, port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::task::spawn_blocking(move || command.status())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("Could not start Chrome: {error}"))?;
    if !status.success() {
        return Err(format!("Could not start Chrome ({status})"));
    }

    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        sleep(POLL_INTERVAL).await;
        if let Some(endpoint) = endpoint(port).await {
            let pid = process(profile)
                .await
                .filter(|(_, found_port)| *found_port == Some(port))
                .map(|(pid, _)| pid)
                .ok_or("Something other than the preview browser answers on its port")?;
            if listener(port).await != Some(pid) {
                return Err(
                    "Something other than the preview browser answers on its port".to_owned(),
                );
            }
            return Ok(Instance {
                pid,
                port,
                endpoint,
            });
        }
    }
    Err(if process(profile).await.is_some() {
        "Chrome runs with the preview profile but its debugging port does not answer; quit that \
         Chrome and try again"
            .to_owned()
    } else {
        "Chrome did not start".to_owned()
    })
}

/// The browser of `profile` when it is already running with a debugging port that answers, as
/// after the app quit without stopping it.
async fn running(profile: &Path) -> Option<Instance> {
    let (pid, port) = process(profile).await?;
    let port = port?;
    if listener(port).await != Some(pid) {
        return None;
    }
    let endpoint = endpoint(port).await?;
    Some(Instance {
        pid,
        port,
        endpoint,
    })
}

async fn process(profile: &Path) -> Option<(u32, Option<u16>)> {
    let mut ps = Command::new("/bin/ps");
    ps.args(["-axww", "-o", "pid=,args="]);
    find_process(&run(ps).await?, profile)
}

/// The process that listens on `port` of the loopback interface.
async fn listener(port: u16) -> Option<u32> {
    let mut lsof = Command::new("/usr/sbin/lsof");
    lsof.args([
        "-nP",
        &format!("-iTCP@127.0.0.1:{port}"),
        "-sTCP:LISTEN",
        "-t",
    ]);
    run(lsof).await?.lines().next()?.trim().parse().ok()
}

async fn run(mut command: Command) -> Option<String> {
    let output = tokio::task::spawn_blocking(move || command.stderr(Stdio::null()).output())
        .await
        .ok()?
        .ok()?;
    String::from_utf8(output.stdout).ok()
}

fn free_port() -> Option<u16> {
    (FIRST_PORT..FIRST_PORT + PORTS_TRIED)
        .find(|port| TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, *port)).is_ok())
}

/// The browser's DevTools WebSocket address, once `/json/version` answers on `port`.
async fn endpoint(port: u16) -> Option<String> {
    let version = timeout(HTTP_TIMEOUT, get(port, "/json/version"))
        .await
        .ok()?
        .ok()?;
    let version: Value = serde_json::from_str(&version).ok()?;
    let endpoint = version.get("webSocketDebuggerUrl")?.as_str()?;
    // Only an endpoint on the port asked about is trusted.
    endpoint
        .starts_with(&format!("ws://127.0.0.1:{port}/"))
        .then(|| endpoint.to_owned())
}

/// The body of a plain HTTP GET to the loopback interface.
async fn get(port: u16, path: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    let mut buffer = [0; 4096];
    // Chrome keeps the connection open after answering, so the body ends where its length says.
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
        if response_complete(&response) {
            break;
        }
    }
    let response = String::from_utf8_lossy(&response);
    let (_, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    Ok(body.to_owned())
}

/// Whether `response` holds the headers and as much of the body as `Content-Length` announces.
fn response_complete(response: &[u8]) -> bool {
    let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&response[..end]);
    let length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });
    length.is_some_and(|length| response.len() >= end + 4 + length)
}

/// Asks the browser with `pid` to quit, as quitting it from its menu would.
pub fn terminate(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: `kill` only sends a signal; the pid is that of the browser this app started.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// Ends the browser `$2` once the process `$1` is gone: asks it to quit, then kills it 5 seconds
/// later. It stops by itself once the browser is gone.
const WATCHDOG_SCRIPT: &str = r#"
while kill -0 "$1" 2>/dev/null && kill -0 "$2" 2>/dev/null; do sleep 2; done
kill -0 "$1" 2>/dev/null && exit 0
kill -TERM "$2" -"$2" 2>/dev/null
i=0
while kill -0 "$2" 2>/dev/null && [ "$i" -lt 10 ]; do sleep 0.5; i=$((i + 1)); done
kill -KILL "$2" -"$2" 2>/dev/null
exit 0
"#;

/// The name the watchdog shows in `ps`.
pub const WATCHDOG_NAME: &str = "hako-browser-watchdog";

/// A small process outside this app that quits the browser should the app end without doing so,
/// as when it crashes or is killed. Dropping it stops it.
pub struct Watchdog(Child);

impl Watchdog {
    /// Watches over the browser `browser` for the process `owner`.
    pub fn start(owner: u32, browser: u32) -> io::Result<Self> {
        Command::new("/bin/sh")
            .args(["-c", WATCHDOG_SCRIPT, WATCHDOG_NAME])
            .args([owner.to_string(), browser.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // A group of its own, so that signals meant for the app's group do not reach it.
            .process_group(0)
            .spawn()
            .map(Self)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // Until it is waited for, its pid cannot have been reused for another process.
        if let Ok(None) = self.0.try_wait()
            && let Ok(group) = i32::try_from(self.0.id())
        {
            // SAFETY: `kill` only sends a signal, to the group the watchdog leads.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.0.wait();
    }
}

/// Whether the process `pid` still runs.
pub fn is_running(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Removes downloads older than [`DOWNLOADS_KEPT_FOR`]. Each download is a directory of its own.
pub fn clean_downloads(directory: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > DOWNLOADS_KEPT_FOR);
        if old {
            let path = entry.path();
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(error) = removed {
                log::debug!("[Mac browser] could not remove {}: {error}", path.display());
            }
        }
    }
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
