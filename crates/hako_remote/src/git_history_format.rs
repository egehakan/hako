//! What `git for-each-ref`, `git rev-list` and `git diff-tree` print for the branch and history
//! methods, read into their shapes. Every field git is asked for ends with a NUL byte, which no
//! name and no message can hold, so a record is a fixed number of fields whatever is in them.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::git_status::StatusEntry;
use crate::protocol::{GitBranch, GitBranchKind, GitFileStatus, GitRef, GitRefKind};

/// The most characters of a subject line that are sent.
pub const SUBJECT_MAX_CHARS: usize = 200;
/// The most bytes of a commit message's body that are sent.
pub const BODY_LIMIT: usize = 16 * 1024;

const LOCAL_PREFIX: &str = "refs/heads/";
const REMOTE_PREFIX: &str = "refs/remotes/";
const TAG_PREFIX: &str = "refs/tags/";
/// What `%(upstream:track)` says of an upstream that is configured but no longer exists.
const GONE: &str = "gone";

/// For `git for-each-ref` over `refs/heads` and `refs/remotes`.
pub const BRANCH_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(committerdate:unix)%00\
     %(contents:subject)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00\
     %(worktreepath)%00";
const BRANCH_FIELDS: usize = 8;

/// For `git for-each-ref` over everything that can point at a commit of the history.
pub const REF_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(symref)%00";
const REF_FIELDS: usize = 4;

/// For `git rev-list`.
pub const COMMIT_FORMAT: &str = "--format=%H%x00%P%x00%aN%x00%at%x00%s%x00";
const COMMIT_FIELDS: usize = 5;
/// [`COMMIT_FORMAT`] and the message after its subject.
pub const COMMIT_WITH_BODY_FORMAT: &str = "--format=%H%x00%P%x00%aN%x00%at%x00%s%x00%b%x00";

/// The records of `fields` NUL-ended fields each in what git printed. Git ends a record with a
/// line break, and `rev-list` starts one with a line of its own, so the first field is what
/// follows the last line break before it.
fn records(output: &[u8], fields: usize) -> Vec<Vec<&[u8]>> {
    let all: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    all.chunks_exact(fields)
        .map(|record| {
            let mut record = record.to_vec();
            let first = record[0];
            let start = first
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |line_break| line_break + 1);
            record[0] = &first[start..];
            record
        })
        .collect()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn some(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

/// Seconds since the epoch, as git prints them, in milliseconds.
fn milliseconds(seconds: &[u8]) -> i64 {
    std::str::from_utf8(seconds)
        .ok()
        .and_then(|seconds| seconds.trim().parse::<i64>().ok())
        .map_or(0, |seconds| seconds.saturating_mul(1000))
}

pub fn subject(line: &[u8]) -> String {
    String::from_utf8_lossy(line)
        .chars()
        .take(SUBJECT_MAX_CHARS)
        .collect()
}

/// The start of a message's body, without the blank lines around it, in at most [`BODY_LIMIT`]
/// bytes.
fn body(text: &[u8]) -> String {
    let text = String::from_utf8_lossy(text);
    let text = text.trim_matches('\n').trim_end();
    let mut end = text.len().min(BODY_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The numbers in `ahead 2, behind 1`, either part of which git leaves out when it is zero.
fn tracking_counts(track: &str) -> (u64, u64) {
    let count = |name: &str| {
        track
            .split(", ")
            .find_map(|part| part.strip_prefix(name)?.trim().parse().ok())
            .unwrap_or(0)
    };
    (count("ahead"), count("behind"))
}

fn short_name(reference: &str) -> &str {
    [LOCAL_PREFIX, REMOTE_PREFIX, TAG_PREFIX]
        .iter()
        .find_map(|prefix| reference.strip_prefix(prefix))
        .unwrap_or(reference)
}

/// The branches in what `git for-each-ref` printed with [`BRANCH_FORMAT`], at most `limit` of
/// them, and whether there are more. A branch whose name is not UTF-8 is left out, as are the
/// symbolic refs of a remote and a remote-tracking branch that a local one tracks. `worktree`
/// is every working tree git names, the one that is asked about included.
pub fn parse_branches(output: &[u8], limit: usize) -> (Vec<GitBranch>, bool) {
    let mut locals = Vec::new();
    let mut remotes = Vec::new();
    let mut tracked = HashSet::new();
    for record in records(output, BRANCH_FIELDS) {
        let Ok(reference) = std::str::from_utf8(record[0]) else {
            continue;
        };
        let branch = |name: &str, kind| GitBranch {
            name: name.to_owned(),
            kind,
            tip: lossy(record[1]),
            time: milliseconds(record[2]),
            subject: subject(record[3]),
            upstream: None,
            ahead: None,
            behind: None,
            worktree: None,
        };
        if let Some(name) = reference.strip_prefix(LOCAL_PREFIX) {
            let upstream = lossy(record[4]);
            let track = lossy(record[5]);
            let mut branch = branch(name, GitBranchKind::Local);
            if !upstream.is_empty() && track != GONE {
                let (ahead, behind) = tracking_counts(&track);
                branch.upstream = Some(short_name(&upstream).to_owned());
                branch.ahead = Some(ahead);
                branch.behind = Some(behind);
                tracked.insert(upstream);
            }
            branch.worktree = some(lossy(record[7]));
            locals.push(branch);
        } else if let Some(name) = reference.strip_prefix(REMOTE_PREFIX) {
            let is_symbolic = !record[6].is_empty();
            if !is_symbolic {
                remotes.push((reference.to_owned(), branch(name, GitBranchKind::Remote)));
            }
        }
    }
    let mut remotes: Vec<GitBranch> = remotes
        .into_iter()
        .filter(|(reference, _)| !tracked.contains(reference))
        .map(|(_, branch)| branch)
        .collect();
    let newest_first = |branch: &GitBranch| (Reverse(branch.time), branch.name.clone());
    locals.sort_by_cached_key(newest_first);
    remotes.sort_by_cached_key(newest_first);
    locals.append(&mut remotes);
    let truncated = locals.len() > limit;
    locals.truncate(limit);
    (locals, truncated)
}

/// The branches and tags in what `git for-each-ref` printed with [`REF_FORMAT`], by the commit
/// each points at. `current` is the full name of the branch that is checked out. The symbolic
/// refs of a remote are left out.
pub fn parse_refs(output: &[u8], current: Option<&str>) -> HashMap<String, Vec<GitRef>> {
    let mut refs: HashMap<String, Vec<GitRef>> = HashMap::new();
    for record in records(output, REF_FIELDS) {
        let Ok(reference) = std::str::from_utf8(record[0]) else {
            continue;
        };
        let is_symbolic = !record[3].is_empty();
        let kind = if current == Some(reference) {
            GitRefKind::Head
        } else if reference.starts_with(LOCAL_PREFIX) {
            GitRefKind::Local
        } else if reference.starts_with(REMOTE_PREFIX) {
            if is_symbolic {
                continue;
            }
            GitRefKind::Remote
        } else if reference.starts_with(TAG_PREFIX) {
            GitRefKind::Tag
        } else {
            continue;
        };
        // An annotated tag is an object of its own, and what it tags is what it points at.
        let commit = if record[2].is_empty() {
            record[1]
        } else {
            record[2]
        };
        refs.entry(lossy(commit)).or_default().push(GitRef {
            name: short_name(reference).to_owned(),
            kind,
        });
    }
    refs
}

/// Puts the refs of one commit in the order they are sent in: where `HEAD` is, then local
/// branches, remote-tracking ones and tags, each by name.
pub fn sort_refs(refs: &mut [GitRef]) {
    refs.sort_by_cached_key(|reference| {
        let rank = match reference.kind {
            GitRefKind::Head => 0,
            GitRefKind::Local => 1,
            GitRefKind::Remote => 2,
            GitRefKind::Tag => 3,
        };
        (rank, reference.name.clone())
    });
}

/// A commit as `git rev-list` printed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommit {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// When it was written, in milliseconds since the epoch.
    pub time: i64,
    /// Empty unless the body was asked for.
    pub body: String,
}

/// The commits in what `git rev-list` printed with [`COMMIT_FORMAT`], or, when `has_body`, with
/// [`COMMIT_WITH_BODY_FORMAT`].
pub fn parse_commits(output: &[u8], has_body: bool) -> Vec<ParsedCommit> {
    let fields = COMMIT_FIELDS + usize::from(has_body);
    records(output, fields)
        .into_iter()
        .map(|record| ParsedCommit {
            id: lossy(record[0]),
            parents: String::from_utf8_lossy(record[1])
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            author: lossy(record[2]),
            time: milliseconds(record[3]),
            subject: subject(record[4]),
            body: record
                .get(COMMIT_FIELDS)
                .copied()
                .map(body)
                .unwrap_or_default(),
        })
        .collect()
}

/// The files in what `git diff-tree -z --name-status` printed, ordered by path. A file whose
/// name is not UTF-8 is left out.
pub fn parse_name_status(output: &[u8]) -> Vec<StatusEntry> {
    let mut files = Vec::new();
    let mut records = output.split(|byte| *byte == 0);
    while let Some(letter) = records.next() {
        let Some(&letter) = letter.first() else {
            continue;
        };
        // A rename and a copy name two files, where the file was and where it is.
        let original = match letter {
            b'R' | b'C' => records.next(),
            _ => None,
        };
        let Some(path) = records.next().and_then(text) else {
            continue;
        };
        let (status, old_path) = match letter {
            b'A' | b'C' => (GitFileStatus::Added, None),
            b'D' => (GitFileStatus::Deleted, None),
            b'R' => match original.and_then(text) {
                Some(original) => (GitFileStatus::Renamed, Some(original)),
                None => continue,
            },
            _ => (GitFileStatus::Modified, None),
        };
        files.push(StatusEntry {
            path,
            old_path,
            status,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn text(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec()).ok()
}

#[cfg(test)]
#[path = "git_history_format_tests.rs"]
mod tests;
