use std::os::unix::fs::PermissionsExt as _;

use tempfile::TempDir;

use super::*;
use crate::git_test_support::{commit_all, environment, git, identify, repository, write};
use crate::protocol::GitSwitchKind;

/// Finds working trees the plain way: every directory that holds a `.git` entry.
fn search(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
            if entry.file_name() == GIT_ENTRY {
                found.push(directory.clone());
            } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
    found
}

fn source_control(limit: Duration) -> SourceControl {
    SourceControl {
        limit,
        environment: environment(),
        ..SourceControl::new(search)
    }
}

/// One folder, a directory beside it, and what answers for them.
struct Fixture {
    _directory: TempDir,
    folder: String,
    outside: String,
    source_control: SourceControl,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // Git names a working tree with every link followed, and a temporary directory is
        // often reached through one.
        let base = fs::canonicalize(directory.path()).unwrap();
        let base = base.to_str().unwrap();
        let (folder, outside) = (format!("{base}/folder"), format!("{base}/outside"));
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir_all(&outside).unwrap();
        Self {
            _directory: directory,
            folder,
            outside,
            source_control: source_control(COMMAND_LIMIT),
        }
    }

    fn folders(&self) -> Vec<String> {
        vec![self.folder.clone()]
    }

    /// Makes a repository in the folder and returns its path.
    fn repository(&self, name: &str) -> String {
        let path = format!("{}/{name}", self.folder);
        repository(&path);
        path
    }

    /// Makes a repository in the folder that tracks a bare one beside the folder, and returns
    /// the paths of both.
    fn repository_with_remote(&self, name: &str) -> (String, String) {
        let path = self.repository(name);
        let remote = format!("{}/{name}.git", self.outside);
        git(
            &self.outside,
            &["init", "-q", "--bare", "-b", "main", &remote],
        );
        git(&path, &["remote", "add", "origin", &remote]);
        git(&path, &["push", "-q", "--set-upstream", "origin", "main"]);
        (path, remote)
    }

    /// Has someone else push a commit to `remote` that sets the contents of `file`.
    fn teammate_pushes(&self, remote: &str, file: &str, content: &str) {
        let clone = format!("{}/teammate", self.outside);
        if Path::new(&clone).exists() {
            git(&clone, &["pull", "-q"]);
        } else {
            git(&self.outside, &["clone", "-q", remote, &clone]);
            identify(&clone);
        }
        write(&clone, file, content);
        commit_all(&clone, "Theirs");
        git(&clone, &["push", "-q"]);
    }

    async fn status(&self, repo: &str) -> GitStatus {
        self.source_control
            .status(&self.folders(), &named_repo(repo), None)
            .await
            .unwrap()
    }

    async fn diff(
        &self,
        repo: &str,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<GitDiff, RpcError> {
        let params = GitDiffParams {
            repo: repo.to_owned(),
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            commit: None,
        };
        self.source_control
            .diff(&self.folders(), &params, None)
            .await
    }

    async fn commit(&self, repo: &str, message: &str) -> Result<GitCommit, RpcError> {
        let params = GitCommitParams {
            repo: repo.to_owned(),
            message: message.to_owned(),
        };
        self.source_control
            .commit(&self.folders(), &params, None)
            .await
    }

    async fn push(&self, repo: &str) -> Result<GitPush, RpcError> {
        self.source_control
            .push(&self.folders(), &named_repo(repo), None)
            .await
    }

    async fn pull(&self, repo: &str) -> Result<GitPull, RpcError> {
        self.source_control
            .pull(&self.folders(), &named_repo(repo), None)
            .await
    }

    async fn discard(
        &self,
        repo: &str,
        path: Option<&str>,
        old_path: Option<&str>,
    ) -> Result<GitDiscard, RpcError> {
        let params = GitDiscardParams {
            repo: repo.to_owned(),
            path: path.map(str::to_owned),
            old_path: old_path.map(str::to_owned),
        };
        self.source_control
            .discard(&self.folders(), &params, None)
            .await
    }
}

fn named_repo(repo: &str) -> GitRepoParams {
    GitRepoParams {
        repo: repo.to_owned(),
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, RpcError>) -> ErrorCode {
    result.unwrap_err().code
}

fn text(diff: Result<GitDiff, RpcError>) -> String {
    match diff.unwrap() {
        GitDiff::Text { diff } => diff,
        other @ (GitDiff::Binary { .. } | GitDiff::TooLarge { .. }) => {
            panic!("expected text, got {other:?}")
        }
    }
}

fn exists(repo: &str, file: &str) -> bool {
    Path::new(repo).join(file).exists()
}

/// A repository whose working tree holds one change of every kind.
fn repository_with_changes(fixture: &Fixture) -> String {
    let repo = fixture.repository("changes");
    write(&repo, "keep.txt", "one\ntwo\n");
    write(&repo, "gone.txt", "bye\n");
    write(&repo, "old.txt", "a\nb\nc\n");
    write(&repo, "image.bin", [0, 1, 2, 3]);
    commit_all(&repo, "Files");

    write(&repo, "keep.txt", "one\n2\nthree\n");
    fs::remove_file(format!("{repo}/gone.txt")).unwrap();
    git(&repo, &["mv", "old.txt", "new.txt"]);
    write(&repo, "staged.txt", "new\n");
    git(&repo, &["add", "staged.txt"]);
    write(&repo, "image.bin", [0, 1, 2, 3, 4, 5]);
    write(&repo, "notes.txt", "a\nb");
    write(&repo, "blob.bin", [7, 0, 7]);
    repo
}

#[tokio::test]
async fn lists_nested_and_linked_working_trees_with_the_folder_they_belong_to() {
    let fixture = Fixture::new();
    let site = fixture.repository("site");
    let nested = fixture.repository("site/vendor/lib");
    let linked = format!("{}/site-redesign", fixture.outside);
    git(&site, &["worktree", "add", "-q", "-b", "redesign", &linked]);
    git(&fixture.folder, &["init", "-q", "--bare", "bare.git"]);

    let repos = fixture
        .source_control
        .repos(&fixture.folders(), None)
        .await
        .repos;

    let paths: Vec<&str> = repos.iter().map(|repo| repo.path.as_str()).collect();
    assert_eq!(paths, [&site, &nested, &linked]);
    assert_eq!(
        repos[0],
        GitRepo {
            path: site.clone(),
            name: "site".to_owned(),
            root: fixture.folder.clone(),
            is_worktree: false,
            main_path: None,
            branch: Some("main".to_owned()),
            head: Some(git(&site, &["rev-parse", "HEAD"])),
            upstream: None,
            ahead: 0,
            behind: 0,
            // The repository nested in it, which git reports as one untracked directory.
            changed: 1,
            operation: None,
        }
    );
    assert!(repos[2].is_worktree);
    assert_eq!(repos[2].main_path.as_ref(), Some(&site));
    assert_eq!(repos[2].branch.as_deref(), Some("redesign"));
    assert_eq!(repos[2].root, fixture.folder);
    assert_eq!(fixture.status(&site).await.files[0].path, "vendor/lib");
}

#[test]
fn reads_the_linked_worktrees_git_lists() {
    let listing = b"worktree /code/site\0HEAD 1\0branch refs/heads/main\0\0\
        worktree /code/redesign\0HEAD 2\0detached\0\0\
        worktree /code/gone\0HEAD 3\0branch refs/heads/x\0prunable gitdir file points nowhere\0\0";
    let of_bare = b"worktree /code/site.git\0bare\0\0worktree /code/topic\0HEAD 2\0detached\0\0";

    assert_eq!(linked_worktrees(listing), ["/code/redesign"]);
    assert_eq!(linked_worktrees(of_bare), ["/code/topic"]);
}

#[test]
fn gives_a_working_tree_the_deepest_folder_that_holds_it() {
    let folders = ["/code".to_owned(), "/code/work".to_owned()];

    assert_eq!(root_of("/code/work/app", &folders), Some(&folders[1]));
    assert_eq!(root_of("/code/work", &folders), Some(&folders[1]));
    assert_eq!(root_of("/code/workshop", &folders), Some(&folders[0]));
    assert_eq!(root_of("/elsewhere/app", &folders), None);
}

#[tokio::test]
async fn describes_the_branch_and_how_it_stands_against_what_it_tracks() {
    let fixture = Fixture::new();
    let (app, remote) = fixture.repository_with_remote("app");
    write(&app, "mine.txt", "mine\n");
    commit_all(&app, "Mine");
    fixture.teammate_pushes(&remote, "theirs.txt", "theirs\n");
    git(&app, &["fetch", "-q"]);
    let unborn = format!("{}/unborn", fixture.folder);
    fs::create_dir_all(&unborn).unwrap();
    git(&unborn, &["init", "-q", "-b", "main"]);

    let tracking = fixture.status(&app).await.repo;
    git(&app, &["checkout", "-q", "--detach"]);
    let detached = fixture.status(&app).await.repo;
    let unborn = fixture.status(&unborn).await.repo;

    assert_eq!(tracking.upstream.as_deref(), Some("origin/main"));
    assert_eq!((tracking.ahead, tracking.behind), (1, 1));
    assert_eq!(detached.branch, None);
    assert_eq!(detached.upstream, None);
    assert_eq!(detached.head, tracking.head);
    assert_eq!(unborn.head, None);
    assert_eq!(unborn.branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn lists_changed_files_with_their_status_and_line_counts() {
    let fixture = Fixture::new();
    let repo = repository_with_changes(&fixture);

    let status = fixture.status(&repo).await;

    let file = |path: &str, status, additions, deletions, binary| GitFile {
        path: path.to_owned(),
        old_path: None,
        status,
        additions,
        deletions,
        binary,
    };
    assert_eq!(
        status.files,
        [
            file("blob.bin", GitFileStatus::Untracked, None, None, true),
            file("gone.txt", GitFileStatus::Deleted, Some(0), Some(1), false),
            file("image.bin", GitFileStatus::Modified, None, None, true),
            file("keep.txt", GitFileStatus::Modified, Some(2), Some(1), false),
            GitFile {
                old_path: Some("old.txt".to_owned()),
                ..file("new.txt", GitFileStatus::Renamed, Some(0), Some(0), false)
            },
            file(
                "notes.txt",
                GitFileStatus::Untracked,
                Some(2),
                Some(0),
                false
            ),
            file("staged.txt", GitFileStatus::Added, Some(1), Some(0), false),
        ]
    );
    assert_eq!(status.repo.changed, 7);
    assert!(!status.truncated);
}

#[tokio::test]
async fn answers_a_diff_as_text_binary_or_too_large() {
    let fixture = Fixture::new();
    let repo = repository_with_changes(&fixture);
    write(&repo, "huge.txt", "line\n".repeat(DIFF_LIMIT / 5 + 1));
    fs::create_dir(format!("{repo}/directory")).unwrap();

    let modified = text(fixture.diff(&repo, "keep.txt", None).await);
    let untracked = text(fixture.diff(&repo, "notes.txt", None).await);
    let renamed = text(fixture.diff(&repo, "new.txt", Some("old.txt")).await);
    let deleted = text(fixture.diff(&repo, "gone.txt", None).await);

    assert!(modified.starts_with("diff --git a/keep.txt b/keep.txt\n"));
    assert!(modified.ends_with("@@ -1,2 +1,3 @@\n one\n-two\n+2\n+three\n"));
    assert!(untracked.contains("--- /dev/null\n+++ b/notes.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n"));
    assert!(renamed.contains("rename from old.txt\nrename to new.txt\n"));
    assert!(!renamed.contains("@@"));
    assert!(deleted.ends_with("@@ -1 +0,0 @@\n-bye\n"));
    assert_eq!(text(fixture.diff(&repo, "README.md", None).await), "");
    assert_eq!(
        fixture.diff(&repo, "image.bin", None).await,
        Ok(GitDiff::Binary {
            old_size: Some(4),
            new_size: Some(6)
        })
    );
    assert_eq!(
        fixture.diff(&repo, "blob.bin", None).await,
        Ok(GitDiff::Binary {
            old_size: None,
            new_size: Some(3)
        })
    );
    assert_eq!(
        fixture.diff(&repo, "huge.txt", None).await,
        Ok(GitDiff::TooLarge {
            limit: DIFF_LIMIT as u64
        })
    );
    assert_eq!(
        code(fixture.diff(&repo, "never.txt", None).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(fixture.diff(&repo, "directory", None).await),
        ErrorCode::Failed
    );
    assert_eq!(
        code(fixture.diff(&repo, "../changes/keep.txt", None).await),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(fixture.diff(&repo, "/keep.txt", None).await),
        ErrorCode::BadRequest
    );
}

#[tokio::test]
async fn commits_everything_and_answers_with_what_git_said() {
    let fixture = Fixture::new();
    let repo = repository_with_changes(&fixture);
    let message = "Change a few things\n\n# kept, since nobody edits this message";

    let committed = fixture.commit(&repo, message).await.unwrap();
    let nothing_left = fixture.commit(&repo, "Again").await.unwrap_err();

    assert_eq!(committed.commit, git(&repo, &["rev-parse", "HEAD"]));
    assert_eq!(committed.repo.head.as_ref(), Some(&committed.commit));
    assert_eq!(committed.repo.changed, 0);
    assert!(committed.output.contains("Change a few things"));
    assert_eq!(git(&repo, &["log", "-1", "--format=%B"]), message);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert_eq!(nothing_left.code, ErrorCode::GitFailed);
    assert!(nothing_left.message.contains("nothing to commit"));
    assert_eq!(
        code(fixture.commit(&repo, " \n").await),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(fixture.commit(&repo, &"x".repeat(MESSAGE_LIMIT + 1)).await),
        ErrorCode::BadRequest
    );
}

#[tokio::test]
async fn pushes_publishes_and_answers_a_rejected_push_with_what_git_said() {
    let fixture = Fixture::new();
    let (app, remote) = fixture.repository_with_remote("app");
    let alone = fixture.repository("alone");
    write(&app, "mine.txt", "mine\n");
    commit_all(&app, "Mine");

    let pushed = fixture.push(&app).await.unwrap();
    git(&app, &["checkout", "-q", "-b", "topic"]);
    let published = fixture.push(&app).await.unwrap();
    git(&app, &["checkout", "-q", "main"]);
    fixture.teammate_pushes(&remote, "theirs.txt", "theirs\n");
    write(&app, "mine.txt", "more\n");
    commit_all(&app, "More");
    let rejected = fixture.push(&app).await.unwrap_err();
    let nowhere = fixture.push(&alone).await.unwrap_err();

    assert!(!pushed.published);
    assert!(pushed.output.contains("main -> main"));
    assert_eq!(pushed.repo.ahead, 0);
    assert!(published.published);
    assert_eq!(published.repo.upstream.as_deref(), Some("origin/topic"));
    assert_eq!(rejected.code, ErrorCode::GitFailed);
    assert!(rejected.message.contains("[rejected]"));
    assert_eq!(git(&remote, &["rev-list", "--count", "main"]), "3");
    assert_eq!(nowhere.code, ErrorCode::Failed);
}

#[tokio::test]
async fn pulls_and_leaves_a_conflict_as_git_left_it() {
    let fixture = Fixture::new();
    let (app, remote) = fixture.repository_with_remote("app");
    git(&app, &["config", "pull.rebase", "false"]);

    fixture.teammate_pushes(&remote, "README.md", "theirs\n");
    let pulled = fixture.pull(&app).await.unwrap();
    fixture.teammate_pushes(&remote, "README.md", "theirs again\n");
    write(&app, "README.md", "mine\n");
    commit_all(&app, "Mine");
    let stopped = fixture.pull(&app).await.unwrap_err();
    let conflicted = fixture.status(&app).await;
    let markers = text(fixture.diff(&app, "README.md", None).await);
    let abandoned = fixture.discard(&app, None, None).await.unwrap();

    assert_eq!(pulled.repo.behind, 0);
    assert_eq!(
        pulled.repo.head,
        Some(git(&remote, &["rev-parse", "main~1"]))
    );
    assert_eq!(stopped.code, ErrorCode::GitFailed);
    assert!(stopped.message.contains("CONFLICT"));
    assert_eq!(conflicted.repo.operation, Some(GitOperation::Merge));
    assert_eq!(conflicted.files[0].status, GitFileStatus::Conflicted);
    assert!(markers.contains("+<<<<<<< HEAD\n"));
    assert_eq!(abandoned.repo.operation, None);
    assert_eq!(abandoned.repo.changed, 0);
}

#[tokio::test]
async fn discards_one_file_and_no_other() {
    let fixture = Fixture::new();
    let repo = repository_with_changes(&fixture);
    write(&repo, "*", "a file named like a pattern\n");

    for (path, old_path) in [
        ("keep.txt", None),
        ("new.txt", Some("old.txt")),
        ("staged.txt", None),
        ("*", None),
        ("README.md", None),
    ] {
        fixture.discard(&repo, Some(path), old_path).await.unwrap();
    }

    assert_eq!(
        fs::read_to_string(format!("{repo}/keep.txt")).unwrap(),
        "one\ntwo\n"
    );
    assert!(exists(&repo, "old.txt"));
    for discarded in ["new.txt", "staged.txt", "*"] {
        assert!(!exists(&repo, discarded), "{discarded} is still there");
    }
    let left: Vec<String> = fixture
        .status(&repo)
        .await
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    assert_eq!(left, ["blob.bin", "gone.txt", "image.bin", "notes.txt"]);
}

#[tokio::test]
async fn discards_every_change_but_only_where_there_is_a_commit_to_go_back_to() {
    let fixture = Fixture::new();
    let repo = repository_with_changes(&fixture);
    fs::create_dir(format!("{repo}/scratch")).unwrap();
    write(&repo, "scratch/draft.txt", "draft\n");
    write(&repo, ".gitignore", "ignored.log\n");
    commit_all(&repo, "Everything");
    write(&repo, "keep.txt", "changed\n");
    write(&repo, "ignored.log", "stays\n");
    fs::create_dir(format!("{repo}/untracked")).unwrap();
    write(&repo, "untracked/file.txt", "goes\n");
    let unborn = format!("{}/unborn", fixture.folder);
    fs::create_dir_all(&unborn).unwrap();
    git(&unborn, &["init", "-q", "-b", "main"]);
    write(&unborn, "first.txt", "first\n");

    let discarded = fixture.discard(&repo, None, None).await.unwrap();
    let nothing_to_go_back_to = fixture.discard(&unborn, None, None).await;

    assert_eq!(discarded.repo.changed, 0);
    assert!(!exists(&repo, "untracked"));
    assert!(exists(&repo, "ignored.log"));
    assert_eq!(code(nothing_to_go_back_to), ErrorCode::Failed);
    assert!(exists(&unborn, "first.txt"));
}

#[tokio::test]
async fn serves_only_the_repositories_it_lists_and_one_change_at_a_time() {
    let fixture = Fixture::new();
    let repo = fixture.repository("listed");
    let unlisted = format!("{}/unlisted", fixture.outside);
    repository(&unlisted);
    write(&unlisted, "README.md", "changed\n");
    let folders = fixture.folders();
    let source_control = &fixture.source_control;

    assert_eq!(
        code(
            source_control
                .status(&folders, &named_repo(&unlisted), None)
                .await
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(fixture.discard(&unlisted, None, None).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        fs::read_to_string(format!("{unlisted}/README.md")).unwrap(),
        "changed\n"
    );
    for malformed in [
        "listed",
        "/a//b",
        &format!("{repo}/"),
        &format!("{repo}/../listed"),
    ] {
        assert_eq!(
            code(
                source_control
                    .status(&folders, &named_repo(malformed), None)
                    .await
            ),
            ErrorCode::BadRequest
        );
    }

    let running = source_control.begin(&repo).unwrap();
    assert_eq!(code(fixture.pull(&repo).await), ErrorCode::Busy);
    assert_eq!(code(fixture.commit(&repo, "Mine").await), ErrorCode::Busy);
    assert!(
        source_control
            .status(&folders, &named_repo(&repo), None)
            .await
            .is_ok()
    );
    drop(running);
    assert!(fixture.discard(&repo, None, None).await.is_ok());
}

#[tokio::test]
async fn answers_for_branches_and_history_only_in_the_repositories_it_lists() {
    let fixture = Fixture::new();
    fixture.repository_with_remote("listed");
    // The folder is reached through a link, so git names the working tree otherwise than the
    // folder does.
    let link = format!("{}/link", fixture.outside);
    std::os::unix::fs::symlink(&fixture.folder, &link).unwrap();
    let folders = vec![link.clone()];
    let repo = format!("{link}/listed");
    git(&repo, &["branch", "feature"]);
    let linked = format!("{}/linked", fixture.outside);
    git(&repo, &["worktree", "add", "-q", &linked, "feature"]);
    let unlisted = format!("{}/unlisted", fixture.outside);
    repository(&unlisted);
    let source_control = &fixture.source_control;
    let switch = |repo: &str, branch: &str| GitSwitchParams {
        repo: repo.to_owned(),
        branch: branch.to_owned(),
        kind: GitSwitchKind::New,
    };
    let log = |repo: &str, limit| GitLogParams {
        repo: repo.to_owned(),
        limit,
        skip: None,
    };
    let show = |repo: &str| GitShowParams {
        repo: repo.to_owned(),
        commit: git(repo, &["rev-parse", "HEAD"]),
    };
    let checked_out_in = |branches: GitBranches, name: &str| {
        let branch = branches
            .branches
            .into_iter()
            .find(|branch| branch.name == name);
        branch.unwrap().worktree
    };

    let branches = source_control
        .branches(&folders, &named_repo(&repo), None)
        .await
        .unwrap();
    let from_linked = source_control
        .branches(&folders, &named_repo(&linked), None)
        .await
        .unwrap();
    assert_eq!(checked_out_in(branches, "feature"), Some(linked.clone()));
    assert_eq!(checked_out_in(from_linked, "main"), Some(repo.clone()));
    let history = source_control
        .log(&folders, &log(&repo, None), None)
        .await
        .unwrap();
    assert_eq!(history.commits.len(), 1);
    let shown = source_control.show(&folders, &show(&repo), None).await;
    assert_eq!(shown.unwrap().commit, history.commits[0]);
    let diff = GitDiffParams {
        repo: repo.clone(),
        path: "README.md".to_owned(),
        old_path: None,
        commit: Some(history.commits[0].id.clone()),
    };
    assert!(
        text(source_control.diff(&folders, &diff, None).await)
            .ends_with("+++ b/README.md\n@@ -0,0 +1 @@\n+first\n")
    );

    for limit in [0, LOG_LIMIT as u64 + 1] {
        assert_eq!(
            code(
                source_control
                    .log(&folders, &log(&repo, Some(limit)), None)
                    .await
            ),
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        code(
            source_control
                .branches(&folders, &named_repo(&unlisted), None)
                .await
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(
            source_control
                .log(&folders, &log(&unlisted, None), None)
                .await
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(source_control.show(&folders, &show(&unlisted), None).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(
            source_control
                .switch(&folders, &switch(&unlisted, "topic"), None)
                .await
        ),
        ErrorCode::NotFound
    );
    assert_eq!(git(&unlisted, &["branch", "--list", "topic"]), "");

    let running = source_control.begin(&repo).unwrap();
    assert_eq!(
        code(
            source_control
                .switch(&folders, &switch(&repo, "topic"), None)
                .await
        ),
        ErrorCode::Busy
    );
    drop(running);
    let switched = source_control
        .switch(&folders, &switch(&repo, "topic"), None)
        .await
        .unwrap();
    assert_eq!(switched.current.as_deref(), Some("topic"));
    assert!(source_control.begin(&repo).is_ok());
}

#[tokio::test]
async fn stops_a_command_that_runs_for_too_long() {
    let fixture = Fixture {
        source_control: source_control(Duration::from_secs(3)),
        ..Fixture::new()
    };
    let repo = fixture.repository("slow");
    let hook = format!("{repo}/.git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\necho checking\nsleep 60\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    write(&repo, "notes.txt", "notes\n");

    let stopped = fixture.commit(&repo, "Never made").await.unwrap_err();

    assert_eq!(stopped.code, ErrorCode::GitFailed);
    assert_eq!(stopped.message, "checking\nStopped after 3 seconds.");
    assert!(!exists(&repo, ".git/index.lock"));
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn answers_with_futures_that_can_move_between_threads() {
    fn sendable(_: impl Send) {}
    let fixture = Fixture::new();
    let folders = fixture.folders();

    sendable(fixture.source_control.repos(&folders, None));
    sendable(fixture.discard("/", None, None));
    sendable(
        fixture
            .source_control
            .branches(&folders, &named_repo("/"), None),
    );
}

#[test]
fn spaces_out_the_announcements_of_one_repository() {
    let mut limiter = ChangeLimiter::default();
    let start = Instant::now();
    let at = |milliseconds| start + Duration::from_millis(milliseconds);

    assert_eq!(limiter.changed_at("/a", at(0)), Announce::Now);
    assert_eq!(limiter.changed_at("/b", at(100)), Announce::Now);
    assert_eq!(
        limiter.changed_at("/a", at(400)),
        Announce::After(Duration::from_millis(600))
    );
    assert_eq!(limiter.changed_at("/a", at(500)), Announce::Covered);
    limiter.announced_late_at("/a", at(1000));
    assert_eq!(
        limiter.changed_at("/a", at(1500)),
        Announce::After(Duration::from_millis(500))
    );
    limiter.announced_late_at("/a", at(2000));
    assert_eq!(limiter.changed_at("/a", at(3000)), Announce::Now);
}
