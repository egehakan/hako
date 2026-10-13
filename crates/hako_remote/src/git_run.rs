//! Runs the `git` command line for a remote client: away from any terminal, without anything
//! that could wait for someone to type, and never for longer than the request it serves may
//! take.

use std::io;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use async_process::Child;
use command::Stdio;
use command::r#async::Command;
use futures::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::time::{Instant, sleep_until, timeout, timeout_at};

use crate::protocol::{ErrorCode, RpcError};

/// The most of git's output that an answer carries: its end, where git says what went wrong.
pub const OUTPUT_LIMIT: usize = 16 * 1024;

/// How long git may run for one request, which is well within the time a client waits for it.
pub const COMMAND_LIMIT: Duration = Duration::from_secs(240);

/// How long git gets to end by itself, and remove its lock files, once it has been told to.
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

/// How long output is still read after git has ended. What git started may hold the pipes open
/// for much longer, as an ssh connection that is kept for later commands does.
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(250);

/// Set for every command. Nobody is there to answer a prompt, an editor or a pager, messages
/// are read as English, and a path is the name of one file even when it holds `*` or starts
/// with `:`.
const ENVIRONMENT: [(&str, &str); 9] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    // An empty program is no program, also when the configuration or `SSH_ASKPASS` names one.
    ("GIT_ASKPASS", ""),
    ("SSH_ASKPASS_REQUIRE", "never"),
    ("GIT_EDITOR", "true"),
    ("GIT_SEQUENCE_EDITOR", "true"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("LC_ALL", "C"),
    ("GIT_LITERAL_PATHSPECS", "1"),
];

/// Never passed on: each of them would make git work somewhere other than the directory it is
/// started in.
const REMOVED_VARIABLES: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_NAMESPACE",
];

/// How git is run for one request.
pub struct Git {
    /// The `PATH` to run git with, when it is not the app's own.
    path: Option<String>,
    /// Set on top of [`ENVIRONMENT`].
    environment: Vec<(String, String)>,
    /// When whatever is still running is stopped.
    deadline: Instant,
    limit: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// Only looks, and so leaves the index alone even where git would refresh it, which keeps
    /// it out of the way of a command the owner runs at the same moment.
    Read,
    Change,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    Status(i32),
    /// Ended by a signal that was not ours.
    Killed,
    /// Not run to its end, for a reason that is not git's.
    Failed(String),
    /// Still running at the deadline.
    Stopped {
        after: Duration,
    },
    /// Stopped for printing more than was asked for.
    Overflowed,
}

/// A git command that has ended.
#[derive(Debug)]
pub struct Run {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    /// Standard output and standard error together, in the order they arrived.
    output: Vec<u8>,
}

impl Run {
    pub fn succeeded(&self) -> bool {
        self.exit == Exit::Status(0)
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Everything git printed, and after it what became of the command when git did not end by
    /// itself.
    fn said(&self) -> String {
        let printed = String::from_utf8_lossy(&self.output);
        let printed = printed.trim();
        let note = match &self.exit {
            Exit::Status(_) | Exit::Overflowed => return printed.to_owned(),
            Exit::Killed => "git was stopped by a signal.".to_owned(),
            Exit::Stopped { after } => format!("Stopped after {} seconds.", after.as_secs()),
            Exit::Failed(error) => format!("git could not be run: {error}"),
        };
        if printed.is_empty() {
            note
        } else {
            format!("{printed}\n{note}")
        }
    }
}

/// Git's output of some commands, one after the other, as an answer carries it.
pub fn output(runs: &[&Run]) -> String {
    let said: Vec<String> = runs
        .iter()
        .map(|run| run.said())
        .filter(|said| !said.is_empty())
        .collect();
    tail(&said.join("\n")).to_owned()
}

/// What to answer when one of `runs` did not succeed: what git said, or why it never ran.
pub fn failure(runs: &[&Run]) -> RpcError {
    let never_ran = runs.iter().any(|run| match run.exit {
        Exit::Failed(_) => true,
        Exit::Status(_) | Exit::Killed | Exit::Stopped { .. } | Exit::Overflowed => false,
    });
    let code = if never_ran {
        ErrorCode::Failed
    } else {
        ErrorCode::GitFailed
    };
    RpcError::new(code, output(runs))
}

/// The end of `text`, trimmed, in at most [`OUTPUT_LIMIT`] bytes.
pub fn tail(text: &str) -> &str {
    let text = text.trim();
    let mut start = text.len().saturating_sub(OUTPUT_LIMIT);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].trim_start()
}

impl Git {
    pub fn new(path: Option<String>, environment: &[(String, String)], limit: Duration) -> Self {
        Self {
            path,
            environment: environment.to_vec(),
            deadline: Instant::now() + limit,
            limit,
        }
    }

    pub async fn read(&self, directory: &Path, args: &[&str]) -> Run {
        self.run(directory, args, None, None, Purpose::Read).await
    }

    /// Like [`Self::read`], but stops git once it has printed more than `limit` bytes to its
    /// standard output.
    pub async fn read_at_most(&self, directory: &Path, args: &[&str], limit: usize) -> Run {
        self.run(directory, args, None, Some(limit), Purpose::Read)
            .await
    }

    /// The standard output of a command that only looks, or what to answer when it failed.
    pub async fn read_stdout(&self, directory: &Path, args: &[&str]) -> Result<Vec<u8>, RpcError> {
        let run = self.read(directory, args).await;
        if run.succeeded() {
            Ok(run.stdout)
        } else {
            Err(failure(&[&run]))
        }
    }

    pub async fn change(&self, directory: &Path, args: &[&str], stdin: Option<&[u8]>) -> Run {
        self.run(directory, args, stdin, None, Purpose::Change)
            .await
    }

    async fn run(
        &self,
        directory: &Path,
        args: &[&str],
        stdin: Option<&[u8]>,
        stdout_limit: Option<usize>,
        purpose: Purpose,
    ) -> Run {
        log::debug!("[Remote access] git {}", args.join(" "));
        let mut command = self.command(directory, args, stdin.is_some(), purpose);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Run {
                    exit: Exit::Failed(error.to_string()),
                    stdout: Vec::new(),
                    output: Vec::new(),
                };
            }
        };

        let mut stdout = Vec::new();
        let mut output = Vec::new();
        let collected = timeout_at(
            self.deadline,
            collect(&mut child, stdin, stdout_limit, &mut stdout, &mut output),
        )
        .await;
        let exit = match collected {
            Ok(Ok(Collected::Ended(status))) => match status.code() {
                Some(code) => Exit::Status(code),
                None => Exit::Killed,
            },
            Ok(Ok(Collected::Overflowed { has_ended })) => {
                if !has_ended {
                    stop(&mut child).await;
                }
                Exit::Overflowed
            }
            Ok(Err(error)) => {
                stop(&mut child).await;
                Exit::Failed(error.to_string())
            }
            Err(_) => {
                stop(&mut child).await;
                Exit::Stopped { after: self.limit }
            }
        };
        Run {
            exit,
            stdout,
            output,
        }
    }

    fn command(
        &self,
        directory: &Path,
        args: &[&str],
        has_stdin: bool,
        purpose: Purpose,
    ) -> Command {
        // A session of its own has no terminal, so nothing git starts can ask at one either,
        // and it is a process group of its own, which is what gets stopped at the deadline.
        #[cfg(unix)]
        let mut command = Command::new_with_session("git");
        #[cfg(not(unix))]
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(directory)
            .stdin(if has_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in REMOVED_VARIABLES {
            command.env_remove(name);
        }
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
        command.envs(ENVIRONMENT);
        match purpose {
            Purpose::Read => {
                command.env("GIT_OPTIONAL_LOCKS", "0");
            }
            Purpose::Change => {}
        }
        command.envs(
            self.environment
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        );
        command
    }
}

/// What became of a command that was not stopped at the deadline.
enum Collected {
    Ended(ExitStatus),
    /// Git printed more to its standard output than was asked for.
    Overflowed {
        has_ended: bool,
    },
}

/// Feeds `stdin` to git and gathers what it prints, until it has ended or has printed more than
/// `stdout_limit` to its standard output.
async fn collect(
    child: &mut Child,
    stdin: Option<&[u8]>,
    stdout_limit: Option<usize>,
    stdout: &mut Vec<u8>,
    output: &mut Vec<u8>,
) -> io::Result<Collected> {
    let input = child.stdin.take();
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();

    let write = async {
        if let (Some(mut pipe), Some(bytes)) = (input, stdin) {
            // A write that fails means git ended before it had read everything, and its status
            // and its output say why.
            let _ = pipe.write_all(bytes).await;
            let _ = pipe.close().await;
        }
    };
    let read = async {
        let mut out_buffer = vec![0; 64 * 1024];
        let mut err_buffer = vec![0; 16 * 1024];
        let mut status = None;
        let mut drained_at = Instant::now();
        let mut is_drained = false;
        loop {
            if let Some(status) = status
                && (is_drained || (out.is_none() && err.is_none()))
            {
                return Ok(Collected::Ended(status));
            }
            tokio::select! {
                read = read_some(&mut out, &mut out_buffer), if out.is_some() => {
                    let bytes = &out_buffer[..read];
                    if bytes.is_empty() {
                        out = None;
                    }
                    stdout.extend_from_slice(bytes);
                    output.extend_from_slice(bytes);
                    if stdout_limit.is_some_and(|limit| stdout.len() > limit) {
                        return Ok(Collected::Overflowed { has_ended: status.is_some() });
                    }
                }
                read = read_some(&mut err, &mut err_buffer), if err.is_some() => {
                    let bytes = &err_buffer[..read];
                    if bytes.is_empty() {
                        err = None;
                    }
                    output.extend_from_slice(bytes);
                }
                ended = child.status(), if status.is_none() => {
                    status = Some(ended?);
                    drained_at = Instant::now() + DRAIN_AFTER_EXIT;
                }
                () = sleep_until(drained_at), if status.is_some() => is_drained = true,
            }
        }
    };
    let ((), collected) = futures::join!(write, read);
    collected
}

/// Reads what is there into `buffer`. Zero bytes mean that nothing more will come.
async fn read_some(pipe: &mut Option<impl AsyncRead + Unpin>, buffer: &mut [u8]) -> usize {
    match pipe {
        Some(pipe) => pipe.read(buffer).await.unwrap_or(0),
        None => 0,
    }
}

/// Ends git and what it started. Git is asked first, so that it can remove its lock files,
/// and made to only when it does not.
async fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        signal_group(child, libc::SIGTERM);
        if timeout(TERMINATION_GRACE, child.status()).await.is_ok() {
            return;
        }
        signal_group(child, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = timeout(TERMINATION_GRACE, child.status()).await;
}

/// Sends a signal to git and to everything it started that stayed in its process group.
#[cfg(unix)]
fn signal_group(child: &Child, signal: libc::c_int) {
    let Ok(group) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    // SAFETY: `killpg` only sends a signal. The group is the one git leads, and git has not
    // been seen to end when this is called, so the number still names that group.
    unsafe {
        libc::killpg(group, signal);
    }
}

#[cfg(test)]
#[path = "git_run_tests.rs"]
mod tests;
