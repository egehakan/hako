//! What the owner's devices are told when a terminal wants them: which change of a coding agent's
//! status is news, the text of each `notify`, and what is kept while the relay is out of reach.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::protocol::{NOTIFY_BODY_MAX_CHARS, NOTIFY_TITLE_MAX_CHARS, Notify, NotifyKind};

/// How long a command must have run for its end to be news.
pub const COMMAND_MIN_DURATION: Duration = Duration::from_secs(30);

/// How old a notification kept while disconnected may be when it is finally sent.
pub const PENDING_MAX_AGE: Duration = Duration::from_secs(60);
/// How many terminals a notification is kept for while disconnected.
pub const PENDING_MAX_SESSIONS: usize = 8;

const ELLIPSIS: char = '…';
const ESC: char = '\u{1b}';
const BEL: char = '\u{7}';

/// What a coding agent waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentWait {
    Permission,
    Question,
    Input,
}

/// A coding agent's status as it reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentReport {
    Working,
    Waiting {
        wait: AgentWait,
        message: Option<String>,
    },
    Finished {
        response: Option<String>,
    },
    Failed {
        error: Option<String>,
        message: Option<String>,
    },
    Cancelled,
}

/// A notification before it is told where it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NotifyKind,
    pub title: String,
    /// The first line of the body.
    pub detail: Option<String>,
}

impl Notice {
    /// The `notify` for a terminal. `place` names the tab and folder the terminal is in.
    pub fn into_notify(
        self,
        session_id: String,
        tab_id: Option<String>,
        place: Option<&str>,
    ) -> Notify {
        Notify {
            kind: self.kind,
            title: one_line(&self.title, NOTIFY_TITLE_MAX_CHARS),
            body: body(self.detail.as_deref(), place),
            session_id,
            tab_id,
        }
    }
}

/// Remembers what each agent last reported, to tell which report is news.
pub struct AgentNotices<K> {
    last: HashMap<K, AgentReport>,
}

impl<K> Default for AgentNotices<K> {
    fn default() -> Self {
        Self {
            last: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash> AgentNotices<K> {
    /// Takes an agent's latest report and returns the notice it calls for, if any.
    pub fn report(&mut self, key: K, agent_name: &str, report: AgentReport) -> Option<Notice> {
        let previous = self.last.insert(key, report.clone());
        if !is_news(previous.as_ref(), &report) {
            return None;
        }
        agent_notice(agent_name, report)
    }

    pub fn forget(&mut self, key: &K) {
        self.last.remove(key);
    }
}

/// Whether going from `previous` to `next` is something to tell: the agent starts waiting, or
/// waits for something new, or has just finished or failed.
fn is_news(previous: Option<&AgentReport>, next: &AgentReport) -> bool {
    match next {
        AgentReport::Working | AgentReport::Cancelled => false,
        AgentReport::Waiting { message, .. } => match previous {
            Some(AgentReport::Waiting {
                message: previous_message,
                ..
            }) => previous_message != message,
            Some(
                AgentReport::Working
                | AgentReport::Finished { .. }
                | AgentReport::Failed { .. }
                | AgentReport::Cancelled,
            )
            | None => true,
        },
        AgentReport::Finished { .. } => !matches!(previous, Some(AgentReport::Finished { .. })),
        AgentReport::Failed { .. } => !matches!(previous, Some(AgentReport::Failed { .. })),
    }
}

fn agent_notice(agent_name: &str, report: AgentReport) -> Option<Notice> {
    let (kind, what, detail) = match report {
        AgentReport::Working | AgentReport::Cancelled => return None,
        AgentReport::Waiting { wait, message } => {
            let what = match wait {
                AgentWait::Permission => "needs your permission",
                AgentWait::Question => "has a question",
                AgentWait::Input => "is waiting for you",
            };
            (NotifyKind::AgentWaiting, what, message)
        }
        AgentReport::Finished { response } => (NotifyKind::AgentDone, "has finished", response),
        AgentReport::Failed { error, message } => {
            let detail = match (error, message) {
                (Some(error), Some(message)) => format!("{error}: {message}"),
                (Some(error), None) => format!("Stopped with an error: {error}"),
                (None, Some(message)) => message,
                (None, None) => "Stopped with an error".to_owned(),
            };
            (NotifyKind::AgentDone, "failed", Some(detail))
        }
    };
    Some(Notice {
        kind,
        title: format!("{agent_name} {what}"),
        detail,
    })
}

/// The notice for a command that ended after running for `duration`.
pub fn command_notice(command: &str, failed: bool, exit_status: i32, duration: Duration) -> Notice {
    let outcome = if failed { "failed" } else { "finished" };
    let command = command.lines().find(|line| !line.trim().is_empty());
    let title = match command {
        Some(command) => {
            let room = NOTIFY_TITLE_MAX_CHARS - outcome.chars().count() - 1;
            format!("{} {outcome}", one_line(command, room))
        }
        None => format!("Command {outcome}"),
    };
    Notice {
        kind: NotifyKind::CommandDone,
        title,
        detail: Some(format!(
            "Took {}, exit status {exit_status}",
            duration_text(duration)
        )),
    }
}

/// Where a terminal is: `<tab title> in <folder name>`, or whichever of the two it has.
pub fn place(tab_title: Option<&str>, folder_name: Option<&str>) -> Option<String> {
    let tab_title = tab_title.map(str::trim).filter(|title| !title.is_empty());
    let folder_name = folder_name.map(str::trim).filter(|name| !name.is_empty());
    match (tab_title, folder_name) {
        (Some(tab_title), Some(folder_name)) => Some(format!("{tab_title} in {folder_name}")),
        (Some(name), None) | (None, Some(name)) => Some(name.to_owned()),
        (None, None) => None,
    }
}

/// `45 s`, `2 min 14 s`, `1 h 3 min`.
pub fn duration_text(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match (hours, minutes, seconds) {
        (0, 0, seconds) => format!("{seconds} s"),
        (0, minutes, 0) => format!("{minutes} min"),
        (0, minutes, seconds) => format!("{minutes} min {seconds} s"),
        (hours, 0, _) => format!("{hours} h"),
        (hours, minutes, _) => format!("{hours} h {minutes} min"),
    }
}

/// The detail and the place on a line each, within the body's limit. The place is kept whole
/// where it can be, and the detail is cut to make room for it.
fn body(detail: Option<&str>, place: Option<&str>) -> String {
    let place = place
        .map(|place| one_line(place, NOTIFY_TITLE_MAX_CHARS))
        .filter(|place| !place.is_empty());
    let room = match &place {
        Some(place) => NOTIFY_BODY_MAX_CHARS - place.chars().count() - 1,
        None => NOTIFY_BODY_MAX_CHARS,
    };
    let detail = detail
        .map(|detail| one_line(detail, room))
        .filter(|detail| !detail.is_empty());
    match (detail, place) {
        (Some(detail), Some(place)) => format!("{detail}\n{place}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => String::new(),
    }
}

/// `text` as one line of plain text of at most `max_chars` characters: terminal escape sequences
/// are taken out, control characters and runs of white space become one space, and a line that
/// is too long ends in `…`.
pub fn one_line(text: &str, max_chars: usize) -> String {
    let mut line = String::with_capacity(text.len().min(max_chars * 4));
    let mut chars = text.chars().peekable();
    let mut pending_space = false;
    while let Some(c) = chars.next() {
        if c == ESC {
            skip_escape_sequence(&mut chars);
            continue;
        }
        if c.is_whitespace() || is_invisible(c) {
            pending_space = true;
            continue;
        }
        if pending_space && !line.is_empty() {
            line.push(' ');
        }
        pending_space = false;
        line.push(c);
    }
    if line.chars().count() <= max_chars {
        return line;
    }
    let mut cut: String = line.chars().take(max_chars.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push(ELLIPSIS);
    cut
}

/// Characters that are not to be shown: controls, line and paragraph separators and direction
/// marks.
fn is_invisible(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// Skips what follows an `ESC`: a CSI sequence up to its final byte, an OSC, DCS, APC or PM
/// string up to its terminator, or the one character of any other escape.
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']' | 'P' | '_' | '^') => {
            while let Some(c) = chars.next() {
                if c == BEL {
                    break;
                }
                if c == ESC && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        Some(_) | None => {}
    }
}

/// The newest notification of each terminal while the relay is out of reach.
#[derive(Default)]
pub struct PendingNotifications {
    /// Oldest first.
    entries: Vec<(Instant, Notify)>,
}

impl PendingNotifications {
    pub fn keep(&mut self, notify: Notify, now: Instant) {
        self.entries
            .retain(|(_, kept)| kept.session_id != notify.session_id);
        self.entries.push((now, notify));
        if self.entries.len() > PENDING_MAX_SESSIONS {
            self.entries.remove(0);
        }
    }

    /// Takes what is kept, leaving out what has grown too old to send.
    pub fn take(&mut self, now: Instant) -> Vec<Notify> {
        self.entries
            .drain(..)
            .filter(|(kept_at, _)| now.saturating_duration_since(*kept_at) <= PENDING_MAX_AGE)
            .map(|(_, notify)| notify)
            .collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
