//! The words blame is shown with: who, when, and where to find the commit.

use std::borrow::Cow;

use chrono::{DateTime, FixedOffset};

use super::porcelain::BlameCommit;

pub const UNCOMMITTED_LABEL: &str = "Not committed yet";
/// Longest commit summary shown after a line; the rest is cut off.
const ANNOTATION_SUMMARY_CHARS: usize = 72;
const SHORT_SHA_CHARS: usize = 7;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Who git says is committing in a repository: its `user.name` and `user.email`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
}

impl GitIdentity {
    /// Whether `commit` was written by this identity. Either the name or the address is enough,
    /// since people commonly keep one name across several addresses.
    pub fn is_author_of(&self, commit: &BlameCommit) -> bool {
        let same_email = self.email.as_deref().is_some_and(|email| {
            !email.trim().is_empty() && email.trim().eq_ignore_ascii_case(commit.author_mail.trim())
        });
        let same_name = self
            .name
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty() && name.trim() == commit.author.trim());
        same_email || same_name
    }
}

/// The author as shown to the person reading the blame.
pub fn author_label<'a>(commit: &'a BlameCommit, identity: &GitIdentity) -> &'a str {
    if identity.is_author_of(commit) {
        "You"
    } else {
        &commit.author
    }
}

/// The text shown after a line: who last changed it, how long ago, and why.
pub fn annotation_text(commit: &BlameCommit, identity: &GitIdentity, now: i64) -> String {
    if commit.is_uncommitted() {
        return UNCOMMITTED_LABEL.to_string();
    }
    let author = author_label(commit, identity);
    let when = relative_time(commit.author_time, now);
    let summary = truncate_chars(commit.summary.trim(), ANNOTATION_SUMMARY_CHARS);
    if summary.is_empty() {
        format!("{author}, {when}")
    } else {
        format!("{author}, {when} \u{2022} {summary}")
    }
}

/// `text` cut to at most `max_chars` characters, the last of them an ellipsis when something was
/// cut.
pub fn truncate_chars(text: &str, max_chars: usize) -> Cow<'_, str> {
    if text.chars().count() <= max_chars {
        return Cow::Borrowed(text);
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    Cow::Owned(format!("{}\u{2026}", kept.trim_end()))
}

/// How long before `now` the moment `then` was, both in seconds since the Unix epoch.
pub fn relative_time(then: i64, now: i64) -> String {
    // A commit dated after now comes from a clock that was ahead; it reads as just made.
    let elapsed = now.saturating_sub(then).max(0);
    let days = elapsed / DAY;
    match elapsed {
        ..MINUTE => "just now".to_string(),
        MINUTE..HOUR => ago(elapsed / MINUTE, "minute"),
        HOUR..DAY => ago(elapsed / HOUR, "hour"),
        _ => match days {
            1 => "yesterday".to_string(),
            ..14 => ago(days, "day"),
            14..60 => ago(days / 7, "week"),
            60..365 => ago(days / 30, "month"),
            _ => ago(days / 365, "year"),
        },
    }
}

fn ago(count: i64, unit: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// The date and time of a commit on its author's clock, such as `7 June 2025 08:09 -05:00`.
pub fn absolute_time(timestamp: i64, tz_minutes: i32) -> Option<String> {
    let offset = FixedOffset::east_opt(tz_minutes.checked_mul(60)?)?;
    let time = DateTime::from_timestamp(timestamp, 0)?.with_timezone(&offset);
    Some(time.format("%-d %B %Y %H:%M %:z").to_string())
}

pub fn short_sha(sha: &str) -> &str {
    sha.get(..SHORT_SHA_CHARS).unwrap_or(sha)
}

/// The page of commit `sha` on GitHub, when `remote_url` is the address of a repository there.
/// Only github.com itself is recognised; nothing is looked up.
pub fn github_commit_url(remote_url: &str, sha: &str) -> Option<String> {
    if sha.is_empty() || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let (owner, repository) = github_repository(remote_url.trim())?;
    Some(format!(
        "https://github.com/{owner}/{repository}/commit/{sha}"
    ))
}

/// The owner and name of the GitHub repository a remote address points to.
fn github_repository(remote_url: &str) -> Option<(&str, &str)> {
    let path = match remote_url.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme, "https" | "http" | "ssh" | "git" | "git+ssh") {
                return None;
            }
            let (authority, path) = rest.split_once('/')?;
            // Credentials written into the address stay out of the link.
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            let host = host.split_once(':').map_or(host, |(host, _port)| host);
            if !is_github_host(host) {
                return None;
            }
            path
        }
        // The scp-like form: `git@github.com:owner/repository.git`.
        None => {
            let (authority, path) = remote_url.split_once(':')?;
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            if !is_github_host(host) {
                return None;
            }
            path
        }
    };

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repository) = path.split_once('/')?;
    (is_github_name(owner) && is_github_name(repository)).then_some((owner, repository))
}

fn is_github_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

fn is_github_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
