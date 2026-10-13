//! What `git status --porcelain=v2 --branch -z` and `git diff --numstat -z` print, read into the
//! shapes of the source control methods. Names are taken as bytes, since `-z` leaves them
//! unquoted, and a file whose name is not UTF-8 is left out.

use std::collections::{BTreeMap, HashMap};

use crate::protocol::GitFileStatus;

/// Value of `branch.head` while `HEAD` is detached.
const DETACHED_HEAD: &str = "(detached)";
/// Value of `branch.oid` before the first commit.
const UNBORN_OID: &str = "(initial)";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    /// Ordered by path.
    pub files: Vec<StatusEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub status: GitFileStatus,
}

impl StatusEntry {
    fn new(path: &[u8], status: GitFileStatus) -> Option<Self> {
        Some(Self {
            path: text(path)?,
            old_path: None,
            status,
        })
    }
}

pub fn parse_status(output: &[u8]) -> Status {
    let mut status = Status::default();
    let mut upstream = None;
    let mut is_tracking = false;
    let mut files = BTreeMap::new();

    let mut records = output.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        let entry = match record {
            [b'#', b' ', header @ ..] => {
                let header = String::from_utf8_lossy(header);
                if let Some(oid) = header.strip_prefix("branch.oid ") {
                    status.head = (oid != UNBORN_OID).then(|| oid.to_owned());
                } else if let Some(head) = header.strip_prefix("branch.head ") {
                    status.branch = (head != DETACHED_HEAD).then(|| head.to_owned());
                } else if let Some(name) = header.strip_prefix("branch.upstream ") {
                    upstream = Some(name.to_owned());
                } else if let Some((ahead, behind)) =
                    header.strip_prefix("branch.ab ").and_then(tracking_counts)
                {
                    is_tracking = true;
                    status.ahead = ahead;
                    status.behind = behind;
                }
                None
            }
            // Git names a directory it does not look into, which is a repository of its own,
            // with a slash at the end.
            [b'?', b' ', path @ ..] => StatusEntry::new(
                path.strip_suffix(b"/").unwrap_or(path),
                GitFileStatus::Untracked,
            ),
            // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
            [b'u', b' ', ..] => {
                StatusEntry::new(fields_from(record, 10), GitFileStatus::Conflicted)
            }
            // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
            [b'1', b' ', x, y, ..] => ordinary_status(*x, *y)
                .and_then(|status| StatusEntry::new(fields_from(record, 8), status)),
            // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>, and the original path as
            // the next record.
            [b'2', b' ', x, y, ..] => {
                let path = fields_from(record, 9);
                let original = records.next().unwrap_or_default();
                let is_copy = *x == b'C' || *y == b'C';
                match (*y == b'D', is_copy) {
                    // The copy is gone again, and what it was copied from never changed.
                    (true, true) => None,
                    // Renamed in the index and deleted since: the old file is what is missing.
                    (true, false) => StatusEntry::new(original, GitFileStatus::Deleted),
                    (false, true) => StatusEntry::new(path, GitFileStatus::Added),
                    (false, false) => text(original).and_then(|original| {
                        Some(StatusEntry {
                            old_path: Some(original),
                            ..StatusEntry::new(path, GitFileStatus::Renamed)?
                        })
                    }),
                }
            }
            _ => None,
        };
        if let Some(entry) = entry {
            put(&mut files, entry);
        }
    }

    // An upstream that is configured but no longer exists gives no counts, and is as good as
    // none.
    status.upstream = upstream.filter(|_| is_tracking);
    status.files = files.into_values().collect();
    status
}

/// Adds a file, which a path deleted in the index and there again as an untracked file is
/// only once: to its owner it is a file that changed.
fn put(files: &mut BTreeMap<String, StatusEntry>, entry: StatusEntry) {
    let reappeared = files.get(&entry.path).is_some_and(|earlier| {
        matches!(
            (earlier.status, entry.status),
            (GitFileStatus::Deleted, GitFileStatus::Untracked)
                | (GitFileStatus::Untracked, GitFileStatus::Deleted)
        )
    });
    let entry = if reappeared {
        StatusEntry {
            status: GitFileStatus::Modified,
            old_path: None,
            ..entry
        }
    } else {
        entry
    };
    files.insert(entry.path.clone(), entry);
}

/// What an ordinary entry is against `HEAD`, its staged (`x`) and its unstaged (`y`) part taken
/// together. `None` for a file that was added to the index and then deleted: `HEAD` never had
/// it and the working tree has not got it.
fn ordinary_status(x: u8, y: u8) -> Option<GitFileStatus> {
    match (x, y) {
        (b'A', b'D') => None,
        (b'D', _) | (_, b'D') => Some(GitFileStatus::Deleted),
        (b'A', _) | (_, b'A') => Some(GitFileStatus::Added),
        _ => Some(GitFileStatus::Modified),
    }
}

/// The two numbers of `+<ahead> -<behind>`.
fn tracking_counts(value: &str) -> Option<(u64, u64)> {
    let (ahead, behind) = value.split_once(' ')?;
    Some((
        ahead.strip_prefix('+')?.parse().ok()?,
        behind.strip_prefix('-')?.parse().ok()?,
    ))
}

/// What follows the first `count` fields of a record, which are separated by single spaces: a
/// path, which may hold spaces itself.
fn fields_from(record: &[u8], count: usize) -> &[u8] {
    record
        .splitn(count + 1, |byte| *byte == b' ')
        .nth(count)
        .unwrap_or_default()
}

fn text(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec()).ok()
}

/// The changed lines of one file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineCounts {
    /// `None` for a binary file, and for one whose lines were not counted.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
}

impl LineCounts {
    pub const BINARY: Self = Self {
        additions: None,
        deletions: None,
        binary: true,
    };
}

/// Reads `git diff --numstat -z` into counts by path, which for a renamed file is the new one.
pub fn parse_numstat(output: &[u8]) -> HashMap<String, LineCounts> {
    let mut counts = HashMap::new();
    let mut records = output.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // A rename has no path of its own: the old and the new one follow as two records.
        let path = if path.is_empty() {
            records.next();
            records.next().unwrap_or_default()
        } else {
            path
        };
        let lines = if additions == b"-" && deletions == b"-" {
            Some(LineCounts::BINARY)
        } else {
            number(additions)
                .zip(number(deletions))
                .map(|(additions, deletions)| LineCounts {
                    additions: Some(additions),
                    deletions: Some(deletions),
                    binary: false,
                })
        };
        if let (Some(path), Some(lines)) = (text(path), lines) {
            counts.insert(path, lines);
        }
    }
    counts
}

fn number(digits: &[u8]) -> Option<u64> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
#[path = "git_status_tests.rs"]
mod tests;
