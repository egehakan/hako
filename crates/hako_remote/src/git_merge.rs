//! Merging a branch into the one that is checked out, abandoning a merge, what an unfinished
//! merge looks like, and the check that keeps a merge from being committed while conflict
//! markers are still in its files. These are plain functions of a working tree's directory: they
//! answer the remote protocol, and a view of the app can call them just the same.

use std::fs::{self, File};
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

use crate::git_history::{Head, LOCAL_BRANCHES, REMOTE_BRANCHES, commit_id};
use crate::git_run::{self, Exit, Git, failure};
use crate::protocol::{
    ErrorCode, GitMerge, GitMergeAbort, GitMergeKind, GitMergeOutcome, GitMergeState, RpcError,
};

const MERGE_HEAD: &str = "MERGE_HEAD";
const MERGE_MSG: &str = "MERGE_MSG";
/// The most paths a refusal to commit names.
const NAMED_PATHS: usize = 20;

/// Merges a branch into the one that is checked out in the working tree at `directory`, as
/// `git merge` does by default, with the message git makes for the branch's short name. Stopping
/// at conflicts is an outcome, not an error: the merge is left unfinished, as git leaves it.
pub async fn merge(
    directory: &Path,
    branch: &str,
    kind: GitMergeKind,
    git: &Git,
) -> Result<GitMerge, RpcError> {
    if branch.is_empty() || branch.contains('\0') {
        return Err(RpcError::bad_request(format!(
            "{branch:?} is not a valid branch name"
        )));
    }
    let head = Head::read(directory, git).await?;
    let Some(current) = head.reference else {
        return Err(RpcError::bad_request(
            "HEAD is detached, so there is no branch to merge into",
        ));
    };
    let (reference, described) = match kind {
        GitMergeKind::Local => (format!("{LOCAL_BRANCHES}/{branch}"), "branch"),
        GitMergeKind::Remote => (
            format!("{REMOTE_BRANCHES}/{branch}"),
            "remote-tracking branch",
        ),
    };
    let missing = || RpcError::not_found(format!("No {described} {branch}"));
    if !is_reference(directory, &reference, git).await? {
        return Err(missing());
    }
    if current == reference {
        return Err(RpcError::bad_request(format!(
            "{branch} is the branch that is checked out"
        )));
    }
    let theirs = commit_id(directory, &reference, git)
        .await?
        .ok_or_else(missing)?;
    let git_dir = git_dir(directory, git).await?;
    let was_merging = git_dir.join(MERGE_HEAD).exists();

    // Git would name the branch by the full name it is given.
    let prepared = git
        .change(
            directory,
            &["fmt-merge-msg"],
            Some(format!("{theirs}\t\t{described} '{branch}' of .\n").as_bytes()),
        )
        .await;
    let message = prepared.stdout_text();
    let mut args = vec!["merge", "--no-edit"];
    if prepared.succeeded() && !message.trim().is_empty() {
        args.extend(["-m", message.trim_end()]);
    }
    args.extend(["--end-of-options", &reference]);
    let merged = git.change(directory, &args, None).await;
    let after = Head::read(directory, git).await?.commit;

    let (outcome, conflicts) = if merged.succeeded() {
        let outcome = if after == head.commit {
            GitMergeOutcome::UpToDate
        } else if after.as_deref() == Some(theirs.as_str()) {
            GitMergeOutcome::FastForward
        } else {
            GitMergeOutcome::Merged
        };
        (outcome, Vec::new())
    } else {
        // Git refused to start, or failed for another reason than the conflicts it stops at.
        if was_merging || !git_dir.join(MERGE_HEAD).exists() {
            return Err(failure(&[&merged]));
        }
        let conflicts = unmerged_paths(directory, git).await?;
        if conflicts.is_empty() {
            return Err(failure(&[&merged]));
        }
        (GitMergeOutcome::Conflicts, conflicts)
    };
    Ok(GitMerge {
        outcome,
        head: after.ok_or_else(|| RpcError::failed("HEAD names no commit after the merge"))?,
        conflicts,
        output: git_run::output(&[&merged]),
    })
}

/// Abandons the merge in progress in the working tree at `directory`, as `git merge --abort`
/// does.
pub async fn merge_abort(directory: &Path, git: &Git) -> Result<GitMergeAbort, RpcError> {
    let git_dir = git_dir(directory, git).await?;
    if !git_dir.join(MERGE_HEAD).exists() {
        return Err(RpcError::bad_request("No merge is in progress"));
    }
    let aborted = git.change(directory, &["merge", "--abort"], None).await;
    if !aborted.succeeded() {
        return Err(failure(&[&aborted]));
    }
    let head = Head::read(directory, git).await?.commit;
    Ok(GitMergeAbort {
        head: head.ok_or_else(|| RpcError::failed("HEAD names no commit after the abort"))?,
    })
}

/// The merge in progress in the working tree at `directory`, if there is one.
pub async fn merge_state(directory: &Path, git: &Git) -> Result<Option<GitMergeState>, RpcError> {
    let git_dir = git_dir(directory, git).await?;
    if !git_dir.join(MERGE_HEAD).exists() {
        return Ok(None);
    }
    let prepared = fs::read_to_string(git_dir.join(MERGE_MSG)).unwrap_or_default();
    Ok(Some(GitMergeState {
        branch: merged_branch(&prepared),
        message: without_comments(&prepared),
        conflicts: unmerged_paths(directory, git).await?,
    }))
}

/// Refuses with `conflict` while a merge is in progress and a file that is unmerged at this
/// moment still holds a conflict marker line. Committing would otherwise stage the markers as
/// the resolution.
pub async fn check_conflict_markers(directory: &Path, git: &Git) -> Result<(), RpcError> {
    let git_dir = git_dir(directory, git).await?;
    if !git_dir.join(MERGE_HEAD).exists() {
        return Ok(());
    }
    let marked: Vec<String> = unmerged_paths(directory, git)
        .await?
        .into_iter()
        .filter(|path| has_conflict_marker(&directory.join(path)))
        .collect();
    if marked.is_empty() {
        Ok(())
    } else {
        Err(RpcError::new(ErrorCode::Conflict, markers_remain(&marked)))
    }
}

async fn git_dir(directory: &Path, git: &Git) -> Result<PathBuf, RpcError> {
    let printed = git
        .read_stdout(
            directory,
            &["rev-parse", "--path-format=absolute", "--git-dir"],
        )
        .await?;
    Ok(PathBuf::from(String::from_utf8_lossy(&printed).trim_end()))
}

/// Whether `reference`, a full ref name, exists exactly as written.
async fn is_reference(directory: &Path, reference: &str, git: &Git) -> Result<bool, RpcError> {
    let shown = git
        .read(directory, &["show-ref", "--verify", "--quiet", reference])
        .await;
    match shown.exit {
        Exit::Status(0) => Ok(true),
        Exit::Status(_) => Ok(false),
        Exit::Killed | Exit::Failed(_) | Exit::Stopped { .. } | Exit::Overflowed => {
            Err(failure(&[&shown]))
        }
    }
}

/// The paths with unmerged entries in the index, relative to the working tree.
async fn unmerged_paths(directory: &Path, git: &Git) -> Result<Vec<String>, RpcError> {
    let listed = git
        .read_stdout(
            directory,
            &["diff", "--name-only", "--diff-filter=U", "-z"],
        )
        .await?;
    let mut paths: Vec<String> = listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    paths.dedup();
    Ok(paths)
}

/// Whether the regular file at `file` has a line that git writes to mark a conflict.
pub(crate) fn has_conflict_marker(file: &Path) -> bool {
    if !fs::symlink_metadata(file).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let Ok(opened) = File::open(file) else {
        return false;
    };
    BufReader::new(opened)
        .split(b'\n')
        .map_while(Result::ok)
        .any(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(&line);
            line.starts_with(b"<<<<<<< ") || line == b"=======" || line.starts_with(b">>>>>>> ")
        })
}

fn markers_remain(paths: &[String]) -> String {
    let mut message = String::from("Conflict markers remain in:");
    for path in paths.iter().take(NAMED_PATHS) {
        message.push('\n');
        message.push_str(path);
    }
    if paths.len() > NAMED_PATHS {
        message.push_str(&format!("\nand {} more", paths.len() - NAMED_PATHS));
    }
    message
}

/// The name of what is being merged, from the first line of git's prepared message, such as
/// `Merge branch 'feature' into main`.
pub(crate) fn merged_branch(prepared: &str) -> Option<String> {
    let first = prepared.lines().next()?.strip_prefix("Merge ")?;
    let (_, quoted) = first.split_once('\'')?;
    let (name, _) = quoted.split_once('\'')?;
    let name = name
        .strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .unwrap_or(name);
    (!name.is_empty()).then(|| name.to_owned())
}

/// Git's prepared message without the lines it means as comments, trimmed.
pub(crate) fn without_comments(prepared: &str) -> String {
    prepared
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
#[path = "git_merge_tests.rs"]
mod tests;
