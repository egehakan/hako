//! Running the system tools the viewers lean on, with a time limit.

use std::io::{self, Read};
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use command::blocking::Command;
use instant::Instant;

const POLL_INTERVAL: Duration = Duration::from_millis(15);

pub(crate) enum RunError {
    Io(io::Error),
    TimedOut,
}

pub(crate) struct Finished {
    pub success: bool,
    pub stdout: String,
}

/// Runs `command` to completion and collects what it prints. The process is killed if it is
/// still running at `deadline`.
pub(crate) fn run(command: &mut Command, deadline: Instant) -> Result<Finished, RunError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(RunError::Io)?;

    // Read on another thread so that a chatty process cannot fill the pipe and stall.
    let stdout = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut output = String::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_string(&mut output);
        }
        output
    });

    let status = loop {
        match child.try_wait().map_err(RunError::Io)? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut);
            }
            None => thread::sleep(POLL_INTERVAL),
        }
    };
    Ok(Finished {
        success: status.success(),
        stdout: reader.join().unwrap_or_default(),
    })
}
