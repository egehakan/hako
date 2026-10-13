//! The branches of a working tree, the switch from one to another, and the history of the one
//! that is checked out, all by the `git` command line. These are plain functions of a working
//! tree's directory: they answer the remote protocol, and a view of the app can call them just
//! the same.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::git::{DIFF, DIFF_LIMIT, EMPTY_TREE, STATUS_LIMIT, file_path, is_binary_diff};
use crate::git_history_format::{
    BRANCH_FORMAT, COMMIT_FORMAT, COMMIT_WITH_BODY_FORMAT, ParsedCommit, REF_FORMAT,
    parse_branches, parse_commits, parse_name_status, parse_refs, sort_refs,
};
use crate::git_run::{Exit, Git, failure};
use crate::git_status::parse_numstat;
use crate::protocol::{
    GitBranches, GitDiff, GitFile, GitLog, GitLogCommit, GitRef, GitRefKind, GitShow, GitSide,
    GitSwitch, GitSwitchKind, RpcError,
};

/// The most branches that are listed.
pub const BRANCH_LIMIT: usize = 500;
/// The most commits one page of the history holds.
pub const LOG_LIMIT: usize = 200;
/// How many commits a page holds when the number is left open.
pub const LOG_DEFAULT: usize = 50;

const LOCAL_BRANCHES: &str = "refs/heads";
const REMOTE_BRANCHES: &str = "refs/remotes";
const TAGS: &str = "refs/tags";
/// What names where `HEAD` is while it is detached.
const DETACHED_HEAD: &str = "HEAD";
/// The branch the one that is checked out tracks.
const UPSTREAM: &str = "@{upstream}";

/// Where `HEAD` is.
struct Head {
    /// The full name of the branch that is checked out; `None` while `HEAD` is detached.
    reference: Option<String>,
    /// `None` before the first commit.
    commit: Option<String>,
}

impl Head {
    async fn read(directory: &Path, git: &Git) -> Result<Self, RpcError> {
        let symbolic = git
            .read(directory, &["symbolic-ref", "--quiet", "HEAD"])
            .await;
        let reference = match symbolic.exit {
            Exit::Status(0) => Some(symbolic.stdout_text().trim().to_owned()),
            // With `--quiet`, this status and no message is how git says that `HEAD` is detached.
            Exit::Status(1) => None,
            Exit::Status(_)
            | Exit::Killed
            | Exit::Failed(_)
            | Exit::Stopped { .. }
            | Exit::Overflowed => return Err(failure(&[&symbolic])),
        };
        Ok(Self {
            reference,
            commit: commit_id(directory, "HEAD", git).await?,
        })
    }

    fn branch(&self) -> Option<String> {
        self.reference.as_deref().map(|reference| {
            reference
                .strip_prefix(LOCAL_BRANCHES)
                .and_then(|name| name.strip_prefix('/'))
                .unwrap_or(reference)
                .to_owned()
        })
    }
}

/// The full id of the commit that `revision` names, or `None` when it names none.
async fn commit_id(
    directory: &Path,
    revision: &str,
    git: &Git,
) -> Result<Option<String>, RpcError> {
    let commit = format!("{revision}^{{commit}}");
    let run = git
        .read(
            directory,
            &[
                "rev-parse",
                "--quiet",
                "--verify",
                "--end-of-options",
                &commit,
            ],
        )
        .await;
    match run.exit {
        Exit::Status(0) => Ok(Some(run.stdout_text().trim().to_owned())),
        Exit::Status(_) => Ok(None),
        Exit::Killed | Exit::Failed(_) | Exit::Stopped { .. } | Exit::Overflowed => {
            Err(failure(&[&run]))
        }
    }
}

/// The commit a client names by its id, in full or abbreviated.
async fn known_commit(directory: &Path, commit: &str, git: &Git) -> Result<String, RpcError> {
    let is_id =
        (4..=64).contains(&commit.len()) && commit.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !is_id {
        return Err(RpcError::bad_request(
            "commit must be a commit id of 4 to 64 hexadecimal digits",
        ));
    }
    commit_id(directory, commit, git)
        .await?
        .ok_or_else(|| RpcError::not_found(format!("No commit {commit} in this repository")))
}

fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The local and the remote-tracking branches of the working tree at `directory`. A branch's
/// `worktree` is as git names it.
pub async fn branches(directory: &Path, git: &Git) -> Result<GitBranches, RpcError> {
    let head = Head::read(directory, git).await?;
    let listed = git
        .read_stdout(
            directory,
            &[
                "for-each-ref",
                BRANCH_FORMAT,
                LOCAL_BRANCHES,
                REMOTE_BRANCHES,
            ],
        )
        .await?;
    let (mut branches, truncated) = parse_branches(&listed, BRANCH_LIMIT);
    let here = real(directory);
    for branch in &mut branches {
        branch.worktree = branch
            .worktree
            .take()
            .filter(|worktree| real(Path::new(worktree)) != here);
    }
    Ok(GitBranches {
        current: head.branch(),
        detached: head.reference.is_none(),
        head: head.commit,
        branches,
        truncated,
    })
}

/// Checks out a branch in the working tree at `directory`, as git does it when nothing is
/// forced: local changes that the switch would overwrite make it fail.
pub async fn switch(
    directory: &Path,
    branch: &str,
    kind: GitSwitchKind,
    git: &Git,
) -> Result<GitSwitch, RpcError> {
    check_branch_name(directory, branch, git).await?;
    let args = match kind {
        GitSwitchKind::Local => vec!["switch", branch],
        GitSwitchKind::Remote => vec!["switch", "--track", branch],
        GitSwitchKind::New => vec!["switch", "-c", branch],
    };
    let switched = git.change(directory, &args, None).await;
    if !switched.succeeded() {
        return Err(failure(&[&switched]));
    }
    let head = Head::read(directory, git).await?;
    Ok(GitSwitch {
        current: head.branch(),
        head: head.commit,
    })
}

/// Fails unless `branch` is a name a branch can have, written out: git reads a name that starts
/// with `-` as an option, and one like `@{-1}` as whatever was checked out before.
async fn check_branch_name(directory: &Path, branch: &str, git: &Git) -> Result<(), RpcError> {
    let invalid = || RpcError::bad_request(format!("{branch:?} is not a valid branch name"));
    if branch.is_empty() || branch.starts_with('-') || branch.contains('\0') {
        return Err(invalid());
    }
    let checked = git
        .read(directory, &["check-ref-format", "--branch", branch])
        .await;
    match checked.exit {
        Exit::Status(0) if checked.stdout_text().trim_end_matches('\n') == branch => Ok(()),
        Exit::Status(_) => Err(invalid()),
        Exit::Killed | Exit::Failed(_) | Exit::Stopped { .. } | Exit::Overflowed => {
            Err(failure(&[&checked]))
        }
    }
}

/// One page of the history of the working tree at `directory`: what can be reached from `HEAD`
/// and from the upstream of its branch, leaving out the `skip` newest commits.
pub async fn log(directory: &Path, limit: usize, skip: u64, git: &Git) -> Result<GitLog, RpcError> {
    let head = Head::read(directory, git).await?;
    let upstream = commit_id(directory, UPSTREAM, git).await?;
    let tips: Vec<&str> = head
        .commit
        .iter()
        .chain(&upstream)
        .map(String::as_str)
        .collect();
    if tips.is_empty() {
        return Ok(GitLog {
            commits: Vec::new(),
            has_more: false,
            head: None,
            upstream: None,
        });
    }
    let skip = format!("--skip={skip}");
    // One more than is asked for tells whether older commits follow.
    let count = format!("--max-count={}", limit.saturating_add(1));
    let mut args = vec![
        "rev-list",
        "--topo-order",
        &skip,
        &count,
        "--encoding=UTF-8",
        COMMIT_FORMAT,
    ];
    args.extend(&tips);
    args.push("--");
    let listed = git.read_stdout(directory, &args).await?;
    let mut parsed = parse_commits(&listed, false);
    let has_more = parsed.len() > limit;
    parsed.truncate(limit);
    let mut decorations = Decorations::read(directory, &head, upstream.as_deref(), git).await?;
    let commits = parsed
        .into_iter()
        .map(|commit| decorations.decorate(commit))
        .collect();
    Ok(GitLog {
        commits,
        has_more,
        head: head.commit,
        upstream,
    })
}

/// One commit of the repository at `directory`, with its whole message and the files it changed
/// against its first parent.
pub async fn show(directory: &Path, commit: &str, git: &Git) -> Result<GitShow, RpcError> {
    let id = known_commit(directory, commit, git).await?;
    let printed = git
        .read_stdout(
            directory,
            &[
                "rev-list",
                "--no-walk",
                "--max-count=1",
                "--encoding=UTF-8",
                COMMIT_WITH_BODY_FORMAT,
                &id,
                "--",
            ],
        )
        .await?;
    let parsed = parse_commits(&printed, true)
        .into_iter()
        .next()
        .ok_or_else(|| RpcError::failed(format!("git did not describe the commit {id}")))?;

    let base = parsed
        .parents
        .first()
        .map_or(EMPTY_TREE, String::as_str)
        .to_owned();
    let changes = |listing| ["diff-tree", "-r", "-M", "-z", listing, &base, &id, "--"];
    let named = git
        .read_stdout(directory, &changes("--name-status"))
        .await?;
    let numstat = git.read(directory, &changes("--numstat")).await;
    // The files are worth listing without their counts.
    let counts = if numstat.succeeded() {
        parse_numstat(&numstat.stdout)
    } else {
        HashMap::new()
    };
    let entries = parse_name_status(&named);
    let truncated = entries.len() > STATUS_LIMIT;
    let files = entries
        .into_iter()
        .take(STATUS_LIMIT)
        .map(|entry| {
            let lines = counts.get(&entry.path).copied().unwrap_or_default();
            GitFile {
                path: entry.path,
                old_path: entry.old_path,
                status: entry.status,
                additions: lines.additions,
                deletions: lines.deletions,
                binary: lines.binary,
            }
        })
        .collect();

    let head = Head::read(directory, git).await?;
    let upstream = commit_id(directory, UPSTREAM, git).await?;
    let mut decorations = Decorations::read(directory, &head, upstream.as_deref(), git).await?;
    let body = parsed.body.clone();
    Ok(GitShow {
        commit: decorations.decorate(parsed),
        body,
        files,
        truncated,
    })
}

/// What one commit changed in one file, against its first parent. `old_path` is where a file
/// that the commit renamed was.
pub async fn commit_diff(
    directory: &Path,
    commit: &str,
    path: &str,
    old_path: Option<&str>,
    git: &Git,
) -> Result<GitDiff, RpcError> {
    let path = file_path(path, "path")?;
    let old_path = old_path
        .map(|old_path| file_path(old_path, "old_path"))
        .transpose()?;
    let id = known_commit(directory, commit, git).await?;
    let base = commit_id(directory, &format!("{id}^1"), git)
        .await?
        .unwrap_or_else(|| EMPTY_TREE.to_owned());
    let after = format!("{id}:{path}");
    let before = format!("{base}:{}", old_path.unwrap_or(path));
    let kind_after = object_kind(directory, &after, git).await;
    let kind_before = object_kind(directory, &before, git).await;
    match kind_after.or(kind_before) {
        None => {
            return Err(RpcError::not_found(format!(
                "Nothing at {path} in the commit {commit}"
            )));
        }
        Some(ObjectKind::Tree) => {
            return Err(RpcError::failed(format!(
                "{path} is a directory, which has no diff of its own"
            )));
        }
        Some(ObjectKind::Blob | ObjectKind::Other) => {}
    }

    let mut args = DIFF.to_vec();
    args.extend(["-M", &base, &id, "--"]);
    args.extend(old_path);
    args.push(path);
    let run = git.read_at_most(directory, &args, DIFF_LIMIT).await;
    match run.exit {
        Exit::Overflowed => {
            return Ok(GitDiff::TooLarge {
                limit: DIFF_LIMIT as u64,
            });
        }
        Exit::Status(0) => {}
        Exit::Status(_) | Exit::Killed | Exit::Failed(_) | Exit::Stopped { .. } => {
            return Err(failure(&[&run]));
        }
    }
    if !is_binary_diff(&run.stdout) {
        return Ok(GitDiff::Text {
            diff: run.stdout_text(),
        });
    }
    Ok(GitDiff::Binary {
        old_size: blob_size(directory, &before, kind_before, git).await,
        new_size: blob_size(directory, &after, kind_after, git).await,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectKind {
    Blob,
    Tree,
    /// A commit, which is what a submodule is in a tree.
    Other,
}

/// What `name` is, when it names anything.
async fn object_kind(directory: &Path, name: &str, git: &Git) -> Option<ObjectKind> {
    let run = git.read(directory, &["cat-file", "-t", name]).await;
    run.succeeded().then(|| match run.stdout_text().trim() {
        "blob" => ObjectKind::Blob,
        "tree" => ObjectKind::Tree,
        _ => ObjectKind::Other,
    })
}

async fn blob_size(
    directory: &Path,
    name: &str,
    kind: Option<ObjectKind>,
    git: &Git,
) -> Option<u64> {
    match kind {
        Some(ObjectKind::Blob) => {
            let size = git.read(directory, &["cat-file", "-s", name]).await;
            size.succeeded()
                .then(|| size.stdout_text().trim().parse().ok())
                .flatten()
        }
        Some(ObjectKind::Tree | ObjectKind::Other) | None => None,
    }
}

/// What a commit is shown with beyond what it holds itself.
struct Decorations {
    /// The branches and tags by the commit they point at.
    refs: HashMap<String, Vec<GitRef>>,
    sides: Sides,
}

/// Which commits only `HEAD` or only its upstream reaches.
enum Sides {
    /// Every commit is on the same side.
    Same(GitSide),
    /// The commits that only one of the two reaches. Every other commit is reached by both.
    Split {
        local: HashSet<String>,
        remote: HashSet<String>,
    },
}

impl Decorations {
    async fn read(
        directory: &Path,
        head: &Head,
        upstream: Option<&str>,
        git: &Git,
    ) -> Result<Self, RpcError> {
        let listed = git
            .read_stdout(
                directory,
                &[
                    "for-each-ref",
                    REF_FORMAT,
                    LOCAL_BRANCHES,
                    REMOTE_BRANCHES,
                    TAGS,
                ],
            )
            .await?;
        let mut refs = parse_refs(&listed, head.reference.as_deref());
        if let (None, Some(commit)) = (&head.reference, &head.commit) {
            refs.entry(commit.clone()).or_default().push(GitRef {
                name: DETACHED_HEAD.to_owned(),
                kind: GitRefKind::Head,
            });
        }

        let sides = match (head.commit.as_deref(), upstream) {
            (Some(head), Some(upstream)) if head != upstream => {
                let range = format!("{head}...{upstream}");
                let marked = git
                    .read_stdout(directory, &["rev-list", "--left-right", &range, "--"])
                    .await?;
                let marked = String::from_utf8_lossy(&marked);
                let reached_by = |mark: char| {
                    marked
                        .lines()
                        .filter_map(|line| line.strip_prefix(mark))
                        .map(str::to_owned)
                        .collect()
                };
                Sides::Split {
                    local: reached_by('<'),
                    remote: reached_by('>'),
                }
            }
            // Nothing is checked out yet, so all there is has still to be pulled.
            (None, Some(_)) => Sides::Same(GitSide::Remote),
            (Some(_), Some(_)) | (Some(_), None) | (None, None) => Sides::Same(GitSide::Both),
        };
        Ok(Self { refs, sides })
    }

    fn decorate(&mut self, commit: ParsedCommit) -> GitLogCommit {
        let mut refs = self.refs.remove(&commit.id).unwrap_or_default();
        sort_refs(&mut refs);
        let side = match &self.sides {
            Sides::Same(side) => *side,
            Sides::Split { local, remote } => {
                if local.contains(&commit.id) {
                    GitSide::Local
                } else if remote.contains(&commit.id) {
                    GitSide::Remote
                } else {
                    GitSide::Both
                }
            }
        };
        GitLogCommit {
            id: commit.id,
            parents: commit.parents,
            subject: commit.subject,
            author: commit.author,
            time: commit.time,
            refs,
            side,
        }
    }
}

#[cfg(test)]
#[path = "git_history_tests.rs"]
mod tests;
