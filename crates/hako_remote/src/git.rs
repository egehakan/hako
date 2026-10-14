//! The source control methods of the remote protocol: the working trees under the owner's
//! folders, what differs from `HEAD` in each, the diff of one file, commit, push, pull and
//! discard, the branches and the history of each, and merging. All of it is done by the `git` command
//! line, in the working trees listed here and nowhere else.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use futures::stream::{self, StreamExt as _};
use tokio::time::Instant;

use crate::files::is_normal;
use crate::git_history::{self, LOG_DEFAULT, LOG_LIMIT};
use crate::git_merge;
use crate::git_run::{self, COMMAND_LIMIT, Exit, Git, failure};
use crate::git_status::{LineCounts, Status, parse_numstat, parse_status};
use crate::protocol::{
    ErrorCode, GitBranches, GitCommit, GitCommitParams, GitDiff, GitDiffParams, GitDiscard,
    GitDiscardParams, GitFile, GitFileStatus, GitLog, GitLogParams, GitMerge, GitMergeAbort,
    GitMergeParams, GitOperation, GitPull, GitPush, GitRepo, GitRepoParams, GitRepos, GitShow,
    GitShowParams, GitStatus, GitSwitch, GitSwitchParams, RpcError,
};

/// The most files `git.status` lists.
pub const STATUS_LIMIT: usize = 2_000;
/// The longest diff that is sent.
pub const DIFF_LIMIT: usize = 1024 * 1024;
/// The longest commit message that is taken.
pub const MESSAGE_LIMIT: usize = 64 * 1024;
/// The least time between two `git.changed` events for the same repository.
pub const CHANGED_EVENT_INTERVAL: Duration = Duration::from_secs(1);

/// How much of a file git looks at to tell whether it is binary.
const BINARY_CHECK_BYTES: u64 = 8_000;
/// How many bytes of untracked files one status reads to count their lines.
const LINE_COUNT_BUDGET: u64 = 64 * 1024 * 1024;
/// What everything is compared with in a repository that has no commit yet. This is the id of
/// the empty tree where objects are named by SHA-1.
pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const CONCURRENT_DESCRIPTIONS: usize = 6;
const GIT_ENTRY: &str = ".git";

const STATUS: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "--branch",
    "-z",
    "--untracked-files=all",
];
/// Enough to know the branch and what it tracks, without looking for untracked files.
const BRANCH_STATUS: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "--branch",
    "-z",
    "--untracked-files=no",
];
/// What makes a diff the same whatever the owner's configuration says.
pub(crate) const DIFF: [&str; 9] = [
    "-c",
    "core.quotePath=false",
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--unified=3",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// Finds the roots of the working trees under a directory.
pub type Search = fn(&Path) -> Vec<PathBuf>;

/// A working tree that is listed.
#[derive(Clone, Debug)]
struct Tree {
    path: String,
    /// The directory of the folder it belongs to.
    root: String,
    /// `path` with every link followed, which is how git names a working tree.
    real: PathBuf,
}

/// The working trees as they were last found.
struct Listing {
    /// The folders that were searched.
    folders: Vec<String>,
    trees: Vec<Tree>,
}

impl Listing {
    fn tree(&self, path: &str) -> Option<&Tree> {
        self.trees.iter().find(|tree| tree.path == path)
    }
}

/// Answers the source control methods, each of which waits for git. Every method takes the
/// directories of the owner's folders, in the order of `workspace.list` and each once, and the
/// `PATH` to run git with when it is not the app's own: the owner's, on which git finds the
/// hooks, filters and credential helpers that the owner's own commands use.
pub struct SourceControl {
    search: Search,
    limit: Duration,
    /// Set for git on top of what it is always given.
    environment: Vec<(String, String)>,
    listing: Mutex<Option<Arc<Listing>>>,
    /// The repositories a command is changing.
    busy: Mutex<HashSet<String>>,
}

impl SourceControl {
    pub fn new(search: Search) -> Self {
        Self {
            search,
            limit: COMMAND_LIMIT,
            environment: Vec::new(),
            listing: Mutex::new(None),
            busy: Mutex::new(HashSet::new()),
        }
    }

    fn git(&self, path_env: Option<String>) -> Git {
        Git::new(path_env, &self.environment, self.limit)
    }

    pub async fn repos(&self, folders: &[String], path_env: Option<String>) -> GitRepos {
        let git = self.git(path_env);
        let (_, repos) = self.list(folders, &git).await;
        GitRepos { repos }
    }

    pub async fn status(
        &self,
        folders: &[String],
        params: &GitRepoParams,
        path_env: Option<String>,
    ) -> Result<GitStatus, RpcError> {
        let git = self.git(path_env);
        let (tree, listing) = self.resolve(folders, &params.repo, &git).await?;
        let directory = Path::new(&tree.path);
        let status = status_of(directory, &STATUS, &[], &git).await?;
        let numstat = git
            .read(directory, &["diff", base(&status), "--numstat", "-M", "-z"])
            .await;
        // The files are worth listing without their counts.
        let counts = if numstat.succeeded() {
            parse_numstat(&numstat.stdout)
        } else {
            HashMap::new()
        };
        let mut budget = LINE_COUNT_BUDGET;
        let files = status
            .files
            .iter()
            .take(STATUS_LIMIT)
            .map(|entry| {
                let lines = match entry.status {
                    GitFileStatus::Untracked => {
                        count_lines(&directory.join(&entry.path), &mut budget)
                    }
                    GitFileStatus::Modified
                    | GitFileStatus::Added
                    | GitFileStatus::Deleted
                    | GitFileStatus::Renamed
                    | GitFileStatus::Conflicted => {
                        counts.get(&entry.path).copied().unwrap_or_default()
                    }
                };
                GitFile {
                    path: entry.path.clone(),
                    old_path: entry.old_path.clone(),
                    status: entry.status,
                    additions: lines.additions,
                    deletions: lines.deletions,
                    binary: lines.binary,
                }
            })
            .collect();
        let truncated = status.files.len() > STATUS_LIMIT;
        let repo = describe(&tree, Some(status), &listing.trees, &git).await?;
        let merge = match repo.operation {
            Some(GitOperation::Merge) => git_merge::merge_state(directory, &git).await?,
            Some(GitOperation::Rebase | GitOperation::CherryPick | GitOperation::Revert) | None => {
                None
            }
        };
        Ok(GitStatus {
            repo,
            files,
            truncated,
            merge,
        })
    }

    pub async fn diff(
        &self,
        folders: &[String],
        params: &GitDiffParams,
        path_env: Option<String>,
    ) -> Result<GitDiff, RpcError> {
        let path = file_path(&params.path, "path")?;
        let old_path = optional_file_path(params.old_path.as_deref(), "old_path")?;
        let git = self.git(path_env);
        let (tree, _) = self.resolve(folders, &params.repo, &git).await?;
        let directory = Path::new(&tree.path);
        if let Some(commit) = &params.commit {
            return git_history::commit_diff(directory, commit, path, old_path, &git).await;
        }
        let status = status_of(directory, &STATUS, &named(path, old_path), &git).await?;
        let entry = status.files.iter().find(|entry| entry.path == path);
        let file = directory.join(path);
        let on_disk = fs::symlink_metadata(&file).ok();
        if on_disk.as_ref().is_some_and(|metadata| metadata.is_dir()) {
            return Err(RpcError::failed(format!(
                "{path} is a directory, which has no diff of its own"
            )));
        }

        let mut args = DIFF.to_vec();
        let is_untracked = match entry.map(|entry| entry.status) {
            Some(GitFileStatus::Untracked) => true,
            Some(
                GitFileStatus::Modified
                | GitFileStatus::Added
                | GitFileStatus::Deleted
                | GitFileStatus::Renamed
                | GitFileStatus::Conflicted,
            ) => false,
            None => {
                if on_disk.is_none() && !is_in_head(directory, &status, path, &git).await {
                    return Err(RpcError::not_found(format!(
                        "Nothing at {path} in this repository"
                    )));
                }
                false
            }
        };
        let renamed_from = entry.and_then(|entry| entry.old_path.as_deref());
        if is_untracked {
            args.extend(["--no-index", "--", "/dev/null", path]);
        } else {
            args.extend(["-M", base(&status), "--"]);
            args.extend(renamed_from);
            args.push(path);
        }
        let run = git.read_at_most(directory, &args, DIFF_LIMIT).await;
        match run.exit {
            Exit::Overflowed => {
                return Ok(GitDiff::TooLarge {
                    limit: DIFF_LIMIT as u64,
                });
            }
            Exit::Status(0) => {}
            // Without an index to compare with, this status only says that the two differ.
            Exit::Status(1) if is_untracked => {}
            Exit::Status(_) | Exit::Killed | Exit::Failed(_) | Exit::Stopped { .. } => {
                return Err(failure(&[&run]));
            }
        }
        if !is_binary_diff(&run.stdout) {
            return Ok(GitDiff::Text {
                diff: run.stdout_text(),
            });
        }
        let old_size = match &status.head {
            Some(_) => {
                let object = format!("HEAD:{}", renamed_from.unwrap_or(path));
                let size = git.read(directory, &["cat-file", "-s", &object]).await;
                size.succeeded()
                    .then(|| size.stdout_text().trim().parse().ok())
                    .flatten()
            }
            None => None,
        };
        Ok(GitDiff::Binary {
            old_size,
            new_size: on_disk
                .filter(|metadata| metadata.is_file())
                .map(|metadata| metadata.len()),
        })
    }

    pub async fn commit(
        &self,
        folders: &[String],
        params: &GitCommitParams,
        path_env: Option<String>,
    ) -> Result<GitCommit, RpcError> {
        if params.message.trim().is_empty() {
            return Err(RpcError::bad_request("message must not be empty"));
        }
        if params.message.len() > MESSAGE_LIMIT {
            return Err(RpcError::bad_request(format!(
                "message must be at most {MESSAGE_LIMIT} bytes"
            )));
        }
        let git = self.git(path_env);
        let (tree, listing) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        let directory = Path::new(&tree.path);
        git_merge::check_conflict_markers(directory, &git).await?;
        let added = git.change(directory, &["add", "-A"], None).await;
        if !added.succeeded() {
            return Err(failure(&[&added]));
        }
        let committed = git
            .change(
                directory,
                &["commit", "-F", "-"],
                Some(params.message.as_bytes()),
            )
            .await;
        if !committed.succeeded() {
            return Err(failure(&[&added, &committed]));
        }
        let commit = git.read_stdout(directory, &["rev-parse", "HEAD"]).await?;
        Ok(GitCommit {
            commit: String::from_utf8_lossy(&commit).trim().to_owned(),
            output: git_run::output(&[&added, &committed]),
            repo: describe(&tree, None, &listing.trees, &git).await?,
        })
    }

    pub async fn push(
        &self,
        folders: &[String],
        params: &GitRepoParams,
        path_env: Option<String>,
    ) -> Result<GitPush, RpcError> {
        let git = self.git(path_env);
        let (tree, listing) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        let directory = Path::new(&tree.path);
        let status = status_of(directory, &BRANCH_STATUS, &[], &git).await?;
        let configured;
        let mut args = vec!["push"];
        let published = match (&status.branch, &status.upstream) {
            (Some(branch), None) => {
                configured = git.read_stdout(directory, &["remote"]).await?;
                let remotes: Vec<&str> = std::str::from_utf8(&configured)
                    .unwrap_or_default()
                    .lines()
                    .filter(|remote| !remote.is_empty())
                    .collect();
                let remote = remotes
                    .iter()
                    .find(|remote| **remote == "origin")
                    .or(remotes.first())
                    .ok_or_else(|| {
                        RpcError::failed(format!(
                            "{} has no remote, so there is nowhere to publish {branch} to",
                            last_segment(&tree.path)
                        ))
                    })?;
                args.extend(["--set-upstream", *remote, branch.as_str()]);
                true
            }
            (None, _) | (Some(_), Some(_)) => false,
        };
        let pushed = git.change(directory, &args, None).await;
        if !pushed.succeeded() {
            return Err(failure(&[&pushed]));
        }
        Ok(GitPush {
            output: git_run::output(&[&pushed]),
            published,
            repo: describe(&tree, None, &listing.trees, &git).await?,
        })
    }

    pub async fn pull(
        &self,
        folders: &[String],
        params: &GitRepoParams,
        path_env: Option<String>,
    ) -> Result<GitPull, RpcError> {
        let git = self.git(path_env);
        let (tree, listing) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        let directory = Path::new(&tree.path);
        let pulled = git.change(directory, &["pull", "--no-edit"], None).await;
        if !pulled.succeeded() {
            return Err(failure(&[&pulled]));
        }
        Ok(GitPull {
            output: git_run::output(&[&pulled]),
            repo: describe(&tree, None, &listing.trees, &git).await?,
        })
    }

    pub async fn discard(
        &self,
        folders: &[String],
        params: &GitDiscardParams,
        path_env: Option<String>,
    ) -> Result<GitDiscard, RpcError> {
        let file = optional_file_path(params.path.as_deref(), "path")?;
        let old_path = optional_file_path(params.old_path.as_deref(), "old_path")?;
        let git = self.git(path_env);
        let (tree, listing) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        let directory = Path::new(&tree.path);
        match file {
            None => {
                let status = status_of(directory, &BRANCH_STATUS, &[], &git).await?;
                if status.head.is_none() {
                    return Err(RpcError::failed(format!(
                        "{} has no commit yet, so there is nothing to go back to",
                        last_segment(&tree.path)
                    )));
                }
                let reset = git
                    .change(directory, &["reset", "--hard", "HEAD"], None)
                    .await;
                // Nothing is deleted from a working tree that could not be put back first.
                if !reset.succeeded() {
                    return Err(failure(&[&reset]));
                }
                let cleaned = git.change(directory, &["clean", "-fd"], None).await;
                if !cleaned.succeeded() {
                    return Err(failure(&[&reset, &cleaned]));
                }
            }
            Some(file) => {
                let status = status_of(directory, &STATUS, &named(file, old_path), &git).await?;
                let entry = status.files.iter().find(|entry| entry.path == file);
                let mut args = Vec::new();
                match entry.map(|entry| entry.status) {
                    Some(GitFileStatus::Untracked) => args.extend(["clean", "-f", "--", file]),
                    Some(
                        GitFileStatus::Modified
                        | GitFileStatus::Added
                        | GitFileStatus::Deleted
                        | GitFileStatus::Renamed
                        | GitFileStatus::Conflicted,
                    ) => {
                        args.extend(["restore", "--source=HEAD", "--staged", "--worktree", "--"]);
                        args.extend(entry.and_then(|entry| entry.old_path.as_deref()));
                        args.push(file);
                    }
                    // It does not differ from `HEAD` any more, so there is nothing to discard.
                    None => {}
                }
                if !args.is_empty() {
                    let discarded = git.change(directory, &args, None).await;
                    if !discarded.succeeded() {
                        return Err(failure(&[&discarded]));
                    }
                }
            }
        }
        Ok(GitDiscard {
            repo: describe(&tree, None, &listing.trees, &git).await?,
        })
    }

    pub async fn branches(
        &self,
        folders: &[String],
        params: &GitRepoParams,
        path_env: Option<String>,
    ) -> Result<GitBranches, RpcError> {
        let git = self.git(path_env);
        let (tree, listing) = self.resolve(folders, &params.repo, &git).await?;
        let mut answer = git_history::branches(Path::new(&tree.path), &git).await?;
        // A working tree goes by the path it is listed under, which is the one a client knows.
        for branch in &mut answer.branches {
            let listed = branch.worktree.as_deref().and_then(|worktree| {
                let real = fs::canonicalize(worktree).ok()?;
                listing.trees.iter().find(|tree| tree.real == real)
            });
            if let Some(listed) = listed {
                branch.worktree = Some(listed.path.clone());
            }
        }
        Ok(answer)
    }

    pub async fn switch(
        &self,
        folders: &[String],
        params: &GitSwitchParams,
        path_env: Option<String>,
    ) -> Result<GitSwitch, RpcError> {
        let git = self.git(path_env);
        let (tree, _) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        git_history::switch(Path::new(&tree.path), &params.branch, params.kind, &git).await
    }

    pub async fn merge(
        &self,
        folders: &[String],
        params: &GitMergeParams,
        path_env: Option<String>,
    ) -> Result<GitMerge, RpcError> {
        let git = self.git(path_env);
        let (tree, _) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        git_merge::merge(Path::new(&tree.path), &params.branch, params.kind, &git).await
    }

    pub async fn merge_abort(
        &self,
        folders: &[String],
        params: &GitRepoParams,
        path_env: Option<String>,
    ) -> Result<GitMergeAbort, RpcError> {
        let git = self.git(path_env);
        let (tree, _) = self.resolve(folders, &params.repo, &git).await?;
        let _busy = self.begin(&tree.path)?;
        git_merge::merge_abort(Path::new(&tree.path), &git).await
    }

    pub async fn log(
        &self,
        folders: &[String],
        params: &GitLogParams,
        path_env: Option<String>,
    ) -> Result<GitLog, RpcError> {
        let limit = match params.limit {
            None => LOG_DEFAULT,
            Some(limit) => usize::try_from(limit)
                .ok()
                .filter(|limit| (1..=LOG_LIMIT).contains(limit))
                .ok_or_else(|| {
                    RpcError::bad_request(format!("limit must be between 1 and {LOG_LIMIT}"))
                })?,
        };
        let git = self.git(path_env);
        let (tree, _) = self.resolve(folders, &params.repo, &git).await?;
        let skip = params.skip.unwrap_or(0);
        git_history::log(Path::new(&tree.path), limit, skip, &git).await
    }

    pub async fn show(
        &self,
        folders: &[String],
        params: &GitShowParams,
        path_env: Option<String>,
    ) -> Result<GitShow, RpcError> {
        let git = self.git(path_env);
        let (tree, _) = self.resolve(folders, &params.repo, &git).await?;
        git_history::show(Path::new(&tree.path), &params.commit, &git).await
    }

    /// Finds the working trees again and describes each. One that git cannot read is left out,
    /// and is not listed from then on.
    async fn list(&self, folders: &[String], git: &Git) -> (Arc<Listing>, Vec<GitRepo>) {
        let found = self.discover(folders, git).await;
        // Boxed, because a stream of futures made by a closure cannot be shown to be `Send`.
        let descriptions: Vec<BoxFuture<'_, Option<GitRepo>>> = found
            .iter()
            .map(|tree| async { describe(tree, None, &found, git).await.ok() }.boxed())
            .collect();
        let described: Vec<Option<GitRepo>> = stream::iter(descriptions)
            .buffered(CONCURRENT_DESCRIPTIONS)
            .collect()
            .await;
        let mut trees = Vec::new();
        let mut repos = Vec::new();
        for (tree, repo) in found.iter().zip(described) {
            if let Some(repo) = repo {
                trees.push(tree.clone());
                repos.push(repo);
            }
        }
        repos.sort_by_cached_key(|repo| {
            (
                folders.iter().position(|folder| *folder == repo.root),
                repo.path.clone(),
            )
        });
        let listing = Arc::new(Listing {
            folders: folders.to_vec(),
            trees,
        });
        *lock(&self.listing) = Some(listing.clone());
        (listing, repos)
    }

    /// Every working tree under the folders, each once, and the linked worktrees of those
    /// repositories, wherever they are.
    async fn discover(&self, folders: &[String], git: &Git) -> Vec<Tree> {
        let mut trees: Vec<Tree> = Vec::new();
        let mut seen = HashSet::new();
        for folder in folders {
            for found in (self.search)(Path::new(folder)) {
                let Some(path) = found.to_str().filter(|path| is_normal(path)) else {
                    continue;
                };
                let real = fs::canonicalize(&found).unwrap_or_else(|_| found.clone());
                if seen.insert(real.clone()) {
                    trees.push(Tree {
                        path: path.to_owned(),
                        root: root_of(path, folders).unwrap_or(folder).to_owned(),
                        real,
                    });
                }
            }
        }

        let mut repositories = HashSet::new();
        for index in 0..trees.len() {
            let Some(common) = common_directory(&trees[index].real) else {
                continue;
            };
            // Asking git costs a process, and most repositories have no linked worktree.
            if !has_linked_worktrees(&common) || !repositories.insert(common) {
                continue;
            }
            let listed = git
                .read(
                    Path::new(&trees[index].path),
                    &["worktree", "list", "--porcelain", "-z"],
                )
                .await;
            if !listed.succeeded() {
                continue;
            }
            for linked in linked_worktrees(&listed.stdout) {
                if !is_normal(linked)
                    || fs::symlink_metadata(Path::new(linked).join(GIT_ENTRY)).is_err()
                {
                    continue;
                }
                let real = fs::canonicalize(linked).unwrap_or_else(|_| PathBuf::from(linked));
                if seen.insert(real.clone()) {
                    trees.push(Tree {
                        path: linked.to_owned(),
                        root: root_of(linked, folders)
                            .unwrap_or(&trees[index].root)
                            .to_owned(),
                        real,
                    });
                }
            }
        }
        trees
    }

    /// The listed working tree that `repo` names. The working trees are found again when the
    /// last search does not know it, or was for other folders.
    async fn resolve(
        &self,
        folders: &[String],
        repo: &str,
        git: &Git,
    ) -> Result<(Tree, Arc<Listing>), RpcError> {
        if !is_normal(repo) {
            return Err(RpcError::bad_request(
                "repo must be an absolute path, with single slashes and without . or .. or a \
                 slash at the end",
            ));
        }
        let missing = || RpcError::not_found(format!("No repository at {repo}"));
        if fs::symlink_metadata(Path::new(repo).join(GIT_ENTRY)).is_err() {
            return Err(missing());
        }
        let known = lock(&self.listing)
            .clone()
            .filter(|listing| listing.folders == folders && listing.tree(repo).is_some());
        let listing = match known {
            Some(listing) => listing,
            None => self.list(folders, git).await.0,
        };
        let tree = listing.tree(repo).ok_or_else(missing)?.clone();
        Ok((tree, listing))
    }

    /// Marks `repo` as being changed until the result is dropped, or fails when it already is.
    fn begin(&self, repo: &str) -> Result<Busy<'_>, RpcError> {
        if lock(&self.busy).insert(repo.to_owned()) {
            Ok(Busy {
                repos: &self.busy,
                repo: repo.to_owned(),
            })
        } else {
            Err(RpcError::new(
                ErrorCode::Busy,
                "Another git command is still running in this repository",
            ))
        }
    }
}

struct Busy<'a> {
    repos: &'a Mutex<HashSet<String>>,
    repo: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        lock(self.repos).remove(&self.repo);
    }
}

/// A panic elsewhere leaves nothing half done in what these locks guard.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn status_of(
    directory: &Path,
    command: &[&str],
    paths: &[&str],
    git: &Git,
) -> Result<Status, RpcError> {
    let mut args = command.to_vec();
    if !paths.is_empty() {
        args.push("--");
        args.extend_from_slice(paths);
    }
    Ok(parse_status(&git.read_stdout(directory, &args).await?))
}

/// What a file is compared with: `HEAD`, or nothing at all before the first commit.
fn base(status: &Status) -> &'static str {
    match status.head {
        Some(_) => "HEAD",
        None => EMPTY_TREE,
    }
}

async fn is_in_head(directory: &Path, status: &Status, path: &str, git: &Git) -> bool {
    match status.head {
        Some(_) => git
            .read(directory, &["cat-file", "-e", &format!("HEAD:{path}")])
            .await
            .succeeded(),
        None => false,
    }
}

/// Describes a working tree. `trees` are the listed ones, among which the working tree a linked
/// one belongs to is looked for.
async fn describe(
    tree: &Tree,
    status: Option<Status>,
    trees: &[Tree],
    git: &Git,
) -> Result<GitRepo, RpcError> {
    let directory = Path::new(&tree.path);
    let status = match status {
        Some(status) => status,
        None => status_of(directory, &STATUS, &[], git).await?,
    };
    let directories = git
        .read_stdout(
            directory,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
                "--git-common-dir",
            ],
        )
        .await?;
    let directories = String::from_utf8_lossy(&directories);
    let mut directories = directories.lines().map(Path::new);
    let (Some(git_dir), Some(common_dir)) = (directories.next(), directories.next()) else {
        return Err(RpcError::failed(format!(
            "git did not say where it keeps {}",
            tree.path
        )));
    };
    let is_worktree = git_dir != common_dir;
    // A bare repository has no working tree of its own for the linked one to belong to.
    let main = common_dir
        .parent()
        .filter(|_| is_worktree && common_dir.file_name().is_some_and(|name| name == GIT_ENTRY));
    Ok(GitRepo {
        path: tree.path.clone(),
        name: last_segment(&tree.path).to_owned(),
        root: tree.root.clone(),
        is_worktree,
        main_path: main.map(|main| {
            trees.iter().find(|listed| listed.real == main).map_or_else(
                || main.to_string_lossy().into_owned(),
                |listed| listed.path.clone(),
            )
        }),
        branch: status.branch,
        head: status.head,
        upstream: status.upstream,
        ahead: status.ahead,
        behind: status.behind,
        changed: status.files.len(),
        operation: unfinished_operation(git_dir),
    })
}

fn unfinished_operation(git_dir: &Path) -> Option<GitOperation> {
    let holds = |name: &str| git_dir.join(name).exists();
    if holds("MERGE_HEAD") {
        Some(GitOperation::Merge)
    } else if holds("rebase-merge") || holds("rebase-apply") {
        Some(GitOperation::Rebase)
    } else if holds("CHERRY_PICK_HEAD") {
        Some(GitOperation::CherryPick)
    } else if holds("REVERT_HEAD") {
        Some(GitOperation::Revert)
    } else {
        None
    }
}

/// The directory of the folder a working tree belongs to: the longest one that is the working
/// tree itself or holds it.
fn root_of<'a>(path: &str, folders: &'a [String]) -> Option<&'a String> {
    folders
        .iter()
        .filter(|folder| {
            path == folder.as_str()
                || folder.as_str() == "/"
                || path
                    .strip_prefix(folder.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|folder| folder.len())
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The directory that holds what all working trees of a repository share, as the files in the
/// working tree's `.git` say.
fn common_directory(tree: &Path) -> Option<PathBuf> {
    let entry = tree.join(GIT_ENTRY);
    let git_dir = if entry.is_dir() {
        entry
    } else {
        let contents = fs::read_to_string(&entry).ok()?;
        tree.join(contents.trim().strip_prefix("gitdir:")?.trim())
    };
    let common = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(relative) => git_dir.join(relative.trim()),
        Err(_) => git_dir,
    };
    fs::canonicalize(common).ok()
}

/// Whether a repository has registered any linked worktree, which git does in this directory.
fn has_linked_worktrees(common: &Path) -> bool {
    fs::read_dir(common.join("worktrees")).is_ok_and(|mut entries| entries.next().is_some())
}

/// The linked worktrees in what `git worktree list --porcelain -z` prints. The first entry is
/// the repository's own working tree, or the repository itself when it is bare.
fn linked_worktrees(listing: &[u8]) -> Vec<&str> {
    let mut worktrees: Vec<(&[u8], bool)> = Vec::new();
    for record in listing.split(|byte| *byte == 0) {
        if let Some(path) = record.strip_prefix(b"worktree ") {
            worktrees.push((path, true));
        } else if record == b"bare" || record.starts_with(b"prunable") {
            // The directory of a worktree that can be pruned is gone.
            if let Some((_, is_there)) = worktrees.last_mut() {
                *is_there = false;
            }
        }
    }
    worktrees
        .into_iter()
        .skip(1)
        .filter(|(_, is_there)| *is_there)
        .filter_map(|(path, _)| std::str::from_utf8(path).ok())
        .collect()
}

/// The lines of a file git does not track, all of which count as added. They are not counted
/// in a file that is too long to be shown as a diff, or once `budget` bytes have been read.
fn count_lines(file: &Path, budget: &mut u64) -> LineCounts {
    let uncounted = LineCounts::default();
    let Ok(metadata) = fs::symlink_metadata(file) else {
        return uncounted;
    };
    if !metadata.is_file() {
        return uncounted;
    }
    let Ok(mut opened) = File::open(file) else {
        return uncounted;
    };
    let mut bytes = Vec::new();
    if (&mut opened)
        .take(BINARY_CHECK_BYTES)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return uncounted;
    }
    if bytes.contains(&0) {
        return LineCounts::BINARY;
    }
    if metadata.len() > (DIFF_LIMIT as u64).min(*budget) || opened.read_to_end(&mut bytes).is_err()
    {
        return uncounted;
    }
    *budget = budget.saturating_sub(bytes.len() as u64);
    let ends = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let unended = u64::from(bytes.last().is_some_and(|last| *last != b'\n'));
    LineCounts {
        additions: Some(ends + unended),
        deletions: Some(0),
        binary: false,
    }
}

/// Whether git printed that two binary files differ where the hunks of a diff would be.
pub(crate) fn is_binary_diff(diff: &[u8]) -> bool {
    diff.split(|byte| *byte == b'\n')
        .take_while(|line| !line.starts_with(b"@@ "))
        .any(|line| line.starts_with(b"Binary files ") && line.ends_with(b" differ"))
}

/// A path inside a repository, if it has the one form the protocol accepts: relative, with
/// single slashes, and without `.`, `..` or a slash at either end.
pub(crate) fn file_path<'a>(path: &'a str, name: &str) -> Result<&'a str, RpcError> {
    let is_relative = path
        .split('/')
        .all(|segment| !matches!(segment, "" | "." | ".."));
    if is_relative && !path.contains('\0') {
        Ok(path)
    } else {
        Err(RpcError::bad_request(format!(
            "{name} must be a path inside the repository, with single slashes and without . or .."
        )))
    }
}

fn optional_file_path<'a>(path: Option<&'a str>, name: &str) -> Result<Option<&'a str>, RpcError> {
    path.map(|path| file_path(path, name)).transpose()
}

/// A file and, when it was renamed, where it was, as paths for git to limit itself to.
fn named<'a>(path: &'a str, old_path: Option<&'a str>) -> Vec<&'a str> {
    old_path.into_iter().chain([path]).collect()
}

/// Whether a command that changes a repository got as far as running git, which may have
/// changed files even when it failed.
pub fn may_have_changed<T>(outcome: &Result<T, RpcError>) -> bool {
    match outcome.as_ref().map_err(|error| error.code) {
        Ok(_) | Err(ErrorCode::GitFailed | ErrorCode::Failed) => true,
        Err(
            ErrorCode::AgentOffline
            | ErrorCode::Timeout
            | ErrorCode::BadRequest
            | ErrorCode::NotFound
            | ErrorCode::NotSupported
            | ErrorCode::NotADirectory
            | ErrorCode::IsADirectory
            | ErrorCode::PermissionDenied
            | ErrorCode::Conflict
            | ErrorCode::TooLarge
            | ErrorCode::Busy,
        ) => false,
    }
}

/// What to do about a change that clients should hear of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Announce {
    Now,
    /// Not before this much time has passed, after which [`ChangeLimiter::announced_late`] has
    /// to be told.
    After(Duration),
    /// Nothing: an announcement that is already waiting will cover this change too.
    Covered,
}

/// Spaces out the `git.changed` events of each repository by [`CHANGED_EVENT_INTERVAL`].
#[derive(Default)]
pub struct ChangeLimiter {
    repos: HashMap<String, Announced>,
}

struct Announced {
    last: Instant,
    is_waiting: bool,
}

impl ChangeLimiter {
    /// Says what to do about a change in `repo` that has just happened.
    pub fn changed(&mut self, repo: &str) -> Announce {
        self.changed_at(repo, Instant::now())
    }

    /// Notes that the announcement that was held back for `repo` is being made.
    pub fn announced_late(&mut self, repo: &str) {
        self.announced_late_at(repo, Instant::now());
    }

    fn changed_at(&mut self, repo: &str, now: Instant) -> Announce {
        let Some(announced) = self.repos.get_mut(repo) else {
            self.repos.insert(
                repo.to_owned(),
                Announced {
                    last: now,
                    is_waiting: false,
                },
            );
            return Announce::Now;
        };
        if announced.is_waiting {
            return Announce::Covered;
        }
        let since = now.saturating_duration_since(announced.last);
        if since >= CHANGED_EVENT_INTERVAL {
            announced.last = now;
            Announce::Now
        } else {
            announced.is_waiting = true;
            Announce::After(CHANGED_EVENT_INTERVAL - since)
        }
    }

    fn announced_late_at(&mut self, repo: &str, now: Instant) {
        if let Some(announced) = self.repos.get_mut(repo) {
            announced.last = now;
            announced.is_waiting = false;
        }
    }
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
