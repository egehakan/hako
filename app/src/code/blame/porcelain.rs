//! Reads the output of `git blame --porcelain` and `git blame --incremental`.
//!
//! Both describe the file as groups of lines: a header `<sha> <original line> <final line>
//! [<line count>]`, then, the first time a commit is mentioned, `key value` lines about it. The
//! porcelain format adds the text of every line, prefixed with a tab, and repeats the header for
//! each line of a group.

use std::collections::HashMap;

/// More lines than any file worth blaming has; a header beyond it is treated as garbage.
const MAX_LINES: usize = 2_000_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameCommit {
    pub sha: String,
    pub author: String,
    /// Without the angle brackets git prints around it.
    pub author_mail: String,
    /// Seconds since the Unix epoch.
    pub author_time: i64,
    /// The author's offset from UTC, in minutes east.
    pub author_tz_minutes: i32,
    /// First line of the commit message.
    pub summary: String,
    /// What the file was called in this commit, which differs from its current name when it has
    /// been renamed since.
    pub path: String,
    /// The history stops at this commit: it is a root commit or the edge of a shallow clone.
    pub is_boundary: bool,
}

impl BlameCommit {
    /// Git attributes lines that are not in `HEAD` to a commit whose id is all zeros.
    pub fn is_uncommitted(&self) -> bool {
        self.sha.bytes().all(|byte| byte == b'0')
    }
}

/// Where one line of the file comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlameLine {
    commit: u32,
    /// The line's number, counted from one, in the commit that introduced it.
    pub original_line: u32,
}

/// The commits behind the lines of one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileBlame {
    lines: Vec<Option<BlameLine>>,
    commits: Vec<BlameCommit>,
}

impl FileBlame {
    /// The commit `line`, counted from zero, comes from. `None` for a line git said nothing
    /// about, which happens past the end of the file and when the blame was cut short.
    pub fn commit_for_line(&self, line: usize) -> Option<&BlameCommit> {
        let blame_line = self.lines.get(line)?.as_ref()?;
        self.commits.get(blame_line.commit as usize)
    }

    pub fn line(&self, line: usize) -> Option<BlameLine> {
        *self.lines.get(line)?
    }

    /// The number of lines that have a commit.
    pub fn blamed_line_count(&self) -> usize {
        self.lines.iter().flatten().count()
    }

    pub fn commits(&self) -> &[BlameCommit] {
        &self.commits
    }
}

/// Parses blame output handed over in pieces of any size, as it arrives from git.
#[derive(Default)]
pub struct BlameParser {
    /// The start of a line whose end has not arrived yet.
    partial_line: Vec<u8>,
    blame: FileBlame,
    commit_by_sha: HashMap<String, u32>,
    /// The commit the `key value` lines being read describe.
    current_commit: Option<u32>,
}

impl BlameParser {
    pub fn feed(&mut self, chunk: &[u8]) {
        let mut rest = chunk;
        while let Some(newline) = rest.iter().position(|byte| *byte == b'\n') {
            let (line, after) = rest.split_at(newline);
            rest = &after[1..];
            if self.partial_line.is_empty() {
                self.parse_line(line);
            } else {
                self.partial_line.extend_from_slice(line);
                let line = std::mem::take(&mut self.partial_line);
                self.parse_line(&line);
            }
        }
        self.partial_line.extend_from_slice(rest);
    }

    /// What has been read so far. Every line it attributes is final, so the result is usable
    /// even when git was stopped before it finished.
    pub fn finish(mut self) -> FileBlame {
        if !self.partial_line.is_empty() {
            let line = std::mem::take(&mut self.partial_line);
            self.parse_line(&line);
        }
        self.blame
    }

    fn parse_line(&mut self, line: &[u8]) {
        // The text of a line of the file. It is told apart by its tab, so text that looks like a
        // header cannot be mistaken for one.
        if line.first() == Some(&b'\t') {
            return;
        }
        if let Some(header) = parse_header(line) {
            self.start_group(header);
            return;
        }
        let Some(commit) = self
            .current_commit
            .and_then(|index| self.blame.commits.get_mut(index as usize))
        else {
            return;
        };

        let line = String::from_utf8_lossy(line);
        let (key, value) = line.split_once(' ').unwrap_or((&line, ""));
        match key {
            "author" => commit.author = value.to_string(),
            "author-mail" => {
                commit.author_mail = value
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string();
            }
            "author-time" => commit.author_time = value.trim().parse().unwrap_or_default(),
            "author-tz" => commit.author_tz_minutes = parse_tz_minutes(value.trim()),
            "summary" => commit.summary = value.to_string(),
            "filename" => commit.path = value.to_string(),
            "boundary" => commit.is_boundary = true,
            // The committer and the previous version of the file are not shown anywhere yet.
            _ => {}
        }
    }

    fn start_group(&mut self, header: GroupHeader) {
        let commit = match self.commit_by_sha.get(&header.sha) {
            Some(index) => *index,
            None => {
                let index = self.blame.commits.len() as u32;
                self.blame.commits.push(BlameCommit {
                    sha: header.sha.clone(),
                    ..Default::default()
                });
                self.commit_by_sha.insert(header.sha, index);
                index
            }
        };
        self.current_commit = Some(commit);

        let Some(first_line) = header.final_line.checked_sub(1) else {
            return;
        };
        let end = first_line.saturating_add(header.line_count);
        if end > MAX_LINES {
            return;
        }
        if self.blame.lines.len() < end {
            self.blame.lines.resize(end, None);
        }
        for (offset, slot) in self.blame.lines[first_line..end].iter_mut().enumerate() {
            *slot = Some(BlameLine {
                commit,
                original_line: header.original_line.saturating_add(offset as u32),
            });
        }
    }
}

struct GroupHeader {
    sha: String,
    original_line: u32,
    /// Counted from one.
    final_line: usize,
    line_count: usize,
}

fn parse_header(line: &[u8]) -> Option<GroupHeader> {
    let line = std::str::from_utf8(line).ok()?;
    let mut fields = line.split(' ');
    let sha = fields.next()?;
    // SHA-1 ids have 40 digits and SHA-256 ids 64.
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let original_line = fields.next()?.parse().ok()?;
    let final_line = fields.next()?.parse().ok()?;
    // Only the first line of a group states how many lines the group has.
    let line_count = match fields.next() {
        Some(count) => count.parse().ok()?,
        None => 1,
    };
    if fields.next().is_some() {
        return None;
    }
    Some(GroupHeader {
        sha: sha.to_string(),
        original_line,
        final_line,
        line_count,
    })
}

/// Converts an offset written the way git writes it, such as `+0530` or `-0800`, to minutes.
fn parse_tz_minutes(value: &str) -> i32 {
    let (sign, digits) = match value.split_at_checked(1) {
        Some(("-", digits)) => (-1, digits),
        Some(("+", digits)) => (1, digits),
        Some(_) | None => (1, value),
    };
    let Ok(number) = digits.parse::<i32>() else {
        return 0;
    };
    sign * ((number / 100) * 60 + number % 100)
}

#[cfg(test)]
#[path = "porcelain_tests.rs"]
mod tests;
