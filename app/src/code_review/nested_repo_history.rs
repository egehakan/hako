//! What the source control overview shows of one repository's history: the commits of its
//! branch as a graph, read a page at a time, and the files of the one commit that is open.
//! Everything that runs git goes through the functions the remote protocol is answered with.

use std::path::Path;
use std::time::Duration;

use ::hako_remote::git_graph::{Graph, GraphRow};
use ::hako_remote::git_history::{self, LOG_DEFAULT};
use ::hako_remote::git_run::{COMMAND_LIMIT, Exit, Git};
use ::hako_remote::protocol::{
    GitBranches, GitFile, GitLog, GitLogCommit, GitShow, GitSwitch, GitSwitchKind,
};
use chrono::{Local, TimeZone};

use crate::code::blame::short_sha;

/// How many commits one page of the history holds.
pub const HISTORY_PAGE: usize = LOG_DEFAULT;

/// The most commits that are read again in one go when a repository changed. A history that
/// was scrolled further than this is cut back to it.
const MAX_RELOADED_COMMITS: usize = 1000;

/// How long git gets for a command that only reads.
const READ_LIMIT: Duration = Duration::from_secs(30);

/// A file of a commit larger than this is not shown as text.
const MAX_SHOWN_FILE_BYTES: usize = 2 * 1024 * 1024;

/// How many lines of a commit's message are shown under it, after the first.
const MAX_BODY_LINES: usize = 8;

fn reader() -> Git {
    Git::new(None, &[], READ_LIMIT)
}

/// A page of history to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: usize,
    pub skip: usize,
}

pub async fn load_page(repo_root: &Path, request: PageRequest) -> Result<GitLog, String> {
    git_history::log(repo_root, request.limit, request.skip as u64, &reader())
        .await
        .map_err(|error| error.message)
}

pub async fn load_branches(repo_root: &Path) -> Result<GitBranches, String> {
    git_history::branches(repo_root, &reader())
        .await
        .map_err(|error| error.message)
}

pub async fn load_commit(repo_root: &Path, commit: &str) -> Result<GitShow, String> {
    git_history::show(repo_root, commit, &reader())
        .await
        .map_err(|error| error.message)
}

/// Checks a branch out as `git switch` does when nothing is forced. `path_env` is the `PATH` of
/// the owner's shell, on which git finds the hooks and filters a checkout runs.
pub async fn switch_branch(
    repo_root: &Path,
    branch: &str,
    kind: GitSwitchKind,
    path_env: Option<String>,
) -> Result<GitSwitch, String> {
    let git = Git::new(path_env, &[], COMMAND_LIMIT);
    git_history::switch(repo_root, branch, kind, &git)
        .await
        .map_err(|error| error.message)
}

/// One side of a file's change in a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileVersion {
    /// The file is not there on this side.
    Absent,
    Text(String),
    Binary,
    TooLarge,
}

/// A file as a commit left it, and as it was in the commit's first parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFileVersions {
    pub before: FileVersion,
    pub after: FileVersion,
}

async fn file_version(
    repo_root: &Path,
    revision: Option<&str>,
    path: &str,
    git: &Git,
) -> FileVersion {
    let Some(revision) = revision else {
        return FileVersion::Absent;
    };
    let object = format!("{revision}:{path}");
    let read = git
        .read_at_most(
            repo_root,
            &["cat-file", "blob", &object],
            MAX_SHOWN_FILE_BYTES,
        )
        .await;
    match read.exit {
        Exit::Status(0) => {
            if read.stdout.contains(&0) {
                FileVersion::Binary
            } else {
                FileVersion::Text(read.stdout_text())
            }
        }
        Exit::Overflowed => FileVersion::TooLarge,
        Exit::Status(_) | Exit::Killed | Exit::Failed(_) | Exit::Stopped { .. } => {
            FileVersion::Absent
        }
    }
}

/// Reads both sides of what `commit` changed in `file`. `parent` is the commit's first parent,
/// which a repository's first commit does not have.
pub async fn load_commit_file(
    repo_root: &Path,
    commit: &str,
    parent: Option<&str>,
    file: &GitFile,
) -> CommitFileVersions {
    let git = reader();
    let before_path = file.old_path.as_deref().unwrap_or(&file.path);
    CommitFileVersions {
        before: file_version(repo_root, parent, before_path, &git).await,
        after: file_version(repo_root, Some(commit), &file.path, &git).await,
    }
}

/// The files of the commit that is open in the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitFiles {
    Loading,
    Loaded(Box<GitShow>),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenCommit {
    pub id: String,
    pub files: CommitFiles,
}

impl OpenCommit {
    /// The lines of text shown under the commit before its files.
    pub fn notes(&self) -> Vec<String> {
        match &self.files {
            CommitFiles::Loading => vec!["Loading the commit\u{2026}".to_owned()],
            CommitFiles::Failed(message) => vec![one_line(message)],
            CommitFiles::Loaded(shown) => {
                let mut notes = vec![commit_summary(&shown.commit)];
                let body: Vec<&str> = shown
                    .body
                    .lines()
                    .map(str::trim_end)
                    .filter(|line| !line.is_empty())
                    .collect();
                notes.extend(
                    body.iter()
                        .take(MAX_BODY_LINES)
                        .map(|line| (*line).to_owned()),
                );
                if body.len() > MAX_BODY_LINES {
                    notes.push("\u{2026}".to_owned());
                }
                if shown.files.is_empty() {
                    notes.push("No files changed".to_owned());
                } else if shown.truncated {
                    notes.push(format!("The first {} files", shown.files.len()));
                }
                notes
            }
        }
    }

    pub fn files(&self) -> &[GitFile] {
        match &self.files {
            CommitFiles::Loaded(shown) => &shown.files,
            CommitFiles::Loading | CommitFiles::Failed(_) => &[],
        }
    }

    /// The first parent of the commit, once the commit has been read.
    pub fn parent(&self) -> Option<&str> {
        match &self.files {
            CommitFiles::Loaded(shown) => shown.commit.parents.first().map(String::as_str),
            CommitFiles::Loading | CommitFiles::Failed(_) => None,
        }
    }
}

/// The abbreviated id of a commit, who wrote it and when, on the clock of this Mac.
fn commit_summary(commit: &GitLogCommit) -> String {
    let written = Local
        .timestamp_millis_opt(commit.time)
        .single()
        .map(|time| time.format("%-d %B %Y %H:%M").to_string());
    [
        Some(short_sha(&commit.id).to_owned()),
        Some(commit.author.clone()),
        written,
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// What git printed, on one line.
pub fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The history of one repository as far as it has been read.
#[derive(Default)]
pub struct RepoHistory {
    commits: Vec<GitLogCommit>,
    rows: Vec<GraphRow>,
    /// The lanes below the last commit read, which the next page continues.
    graph: Option<Graph>,
    head: Option<String>,
    upstream: Option<String>,
    has_more: bool,
    /// How many lanes the widest row has.
    lane_count: usize,
    has_loaded: bool,
    error: Option<String>,
    is_loading: bool,
    reload_when_done: bool,
    open: Option<OpenCommit>,
}

impl RepoHistory {
    pub fn commits(&self) -> &[GitLogCommit] {
        &self.commits
    }

    pub fn rows(&self) -> &[GraphRow] {
        &self.rows
    }

    pub fn lane_count(&self) -> usize {
        self.lane_count
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn is_loading(&self) -> bool {
        self.is_loading
    }

    pub fn open(&self) -> Option<&OpenCommit> {
        self.open.as_ref()
    }

    /// Where the open commit is in the history.
    pub fn open_index(&self) -> Option<usize> {
        let open = self.open.as_ref()?;
        self.commits.iter().position(|commit| commit.id == open.id)
    }

    /// The line shown after the last commit, when there is something to say there.
    pub fn note(&self) -> Option<String> {
        if let Some(error) = &self.error {
            Some(one_line(error))
        } else if !self.has_loaded {
            Some("Loading history\u{2026}".to_owned())
        } else if self.commits.is_empty() {
            Some("No commits yet".to_owned())
        } else if self.has_more {
            Some("Loading more\u{2026}".to_owned())
        } else {
            None
        }
    }

    /// Starts reading the history again from its newest commit, as far as it had been read.
    /// `None` while a page is being read, after which the caller is told to reload.
    pub fn begin_reload(&mut self) -> Option<PageRequest> {
        if self.is_loading {
            self.reload_when_done = true;
            return None;
        }
        self.is_loading = true;
        Some(PageRequest {
            limit: self.commits.len().clamp(HISTORY_PAGE, MAX_RELOADED_COMMITS),
            skip: 0,
        })
    }

    /// Starts reading the page after the commits read so far, when there is one and nothing is
    /// being read.
    pub fn begin_next_page(&mut self) -> Option<PageRequest> {
        if self.is_loading || !self.has_more || self.error.is_some() {
            return None;
        }
        self.is_loading = true;
        Some(PageRequest {
            limit: HISTORY_PAGE,
            skip: self.commits.len(),
        })
    }

    /// Takes in the page that `request` asked for, and closes the open commit when the history
    /// no longer has it. Returns whether the history has to be read again from its start: the
    /// repository changed while the page was read, or the page belongs to a history that has
    /// moved since.
    pub fn apply(&mut self, request: PageRequest, result: Result<GitLog, String>) -> bool {
        self.is_loading = false;
        let mut reload = std::mem::take(&mut self.reload_when_done);
        match result {
            Ok(log) if request.skip == 0 => {
                let mut graph = Graph::new(log.head.as_deref(), log.upstream.as_deref());
                self.rows = graph.extend(&log.commits);
                self.graph = Some(graph);
                self.commits = log.commits;
                self.head = log.head;
                self.upstream = log.upstream;
                self.has_more = log.has_more;
                self.has_loaded = true;
                self.error = None;
            }
            Ok(log) => {
                let is_same_history = log.head == self.head
                    && log.upstream == self.upstream
                    && request.skip == self.commits.len();
                match &mut self.graph {
                    Some(graph) if is_same_history => {
                        self.rows.extend(graph.extend(&log.commits));
                        self.commits.extend(log.commits);
                        self.has_more = log.has_more;
                    }
                    Some(_) | None => reload = true,
                }
            }
            Err(error) => {
                self.has_loaded = true;
                self.error = Some(error);
            }
        }
        self.lane_count = self.rows.iter().map(GraphRow::width).max().unwrap_or(0);
        if self.open_index().is_none() {
            self.open = None;
        }
        reload
    }

    /// Opens the commit, or closes it when it is the open one. Returns whether it is open now,
    /// in which case its files have to be read.
    pub fn toggle_commit(&mut self, id: &str) -> bool {
        if self.open.as_ref().is_some_and(|open| open.id == id) {
            self.open = None;
            return false;
        }
        self.open = Some(OpenCommit {
            id: id.to_owned(),
            files: CommitFiles::Loading,
        });
        true
    }

    /// Takes in what was read of the commit `id`, unless another one has been opened since.
    pub fn set_commit_files(&mut self, id: &str, result: Result<GitShow, String>) {
        if let Some(open) = self.open.as_mut().filter(|open| open.id == id) {
            open.files = match result {
                Ok(shown) => CommitFiles::Loaded(Box::new(shown)),
                Err(error) => CommitFiles::Failed(error),
            };
        }
    }
}

#[cfg(test)]
#[path = "nested_repo_history_tests.rs"]
mod tests;
