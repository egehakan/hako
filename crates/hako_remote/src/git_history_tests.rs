use tempfile::TempDir;

use super::*;
use crate::git_run::COMMAND_LIMIT;
use crate::git_test_support::{
    commit_all, commit_all_at, environment, git, git_at, identify, write,
};
use crate::protocol::{ErrorCode, GitBranch, GitBranchKind, GitFileStatus};

/// A repository `app` that tracks a bare one, in a directory that goes away with this.
struct Fixture {
    _directory: TempDir,
    base: String,
    app: String,
    remote: String,
    git: Git,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // Git names a working tree with every link followed, and a temporary directory is
        // often reached through one.
        let base = fs::canonicalize(directory.path()).unwrap();
        let base = base.to_str().unwrap().to_owned();
        let (app, remote) = (format!("{base}/app"), format!("{base}/app.git"));
        fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        identify(&app);
        write(&app, "README.md", "first\n");
        commit_all_at(&app, "First", 1_000);
        git(&base, &["init", "-q", "--bare", "-b", "main", &remote]);
        git(&app, &["remote", "add", "origin", &remote]);
        git(&app, &["push", "-q", "--set-upstream", "origin", "main"]);
        Self {
            _directory: directory,
            base,
            app,
            remote,
            git: Git::new(None, &environment(), COMMAND_LIMIT),
        }
    }

    fn app(&self) -> &Path {
        Path::new(&self.app)
    }

    /// Has someone else push a commit to the remote that adds `file`, and fetches it.
    fn teammate_pushes(&self, file: &str) {
        let clone = format!("{}/teammate", self.base);
        git(&self.base, &["clone", "-q", &self.remote, &clone]);
        identify(&clone);
        write(&clone, file, "theirs\n");
        commit_all(&clone, "Theirs");
        git(&clone, &["push", "-q"]);
        git(&self.app, &["fetch", "-q"]);
    }

    fn id(&self, revision: &str) -> String {
        git(&self.app, &["rev-parse", revision])
    }

    async fn branches(&self) -> GitBranches {
        branches(self.app(), &self.git).await.unwrap()
    }

    async fn switch(&self, branch: &str, kind: GitSwitchKind) -> Result<GitSwitch, RpcError> {
        switch(self.app(), branch, kind, &self.git).await
    }

    async fn log(&self, limit: usize, skip: u64) -> GitLog {
        log(self.app(), limit, skip, &self.git).await.unwrap()
    }

    async fn show(&self, commit: &str) -> Result<GitShow, RpcError> {
        show(self.app(), commit, &self.git).await
    }

    async fn diff(
        &self,
        commit: &str,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<GitDiff, RpcError> {
        commit_diff(self.app(), commit, path, old_path, &self.git).await
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

fn named<'a>(branches: &'a [GitBranch], name: &str) -> &'a GitBranch {
    branches.iter().find(|branch| branch.name == name).unwrap()
}

fn commit<'a>(log: &'a GitLog, subject: &str) -> &'a GitLogCommit {
    log.commits
        .iter()
        .find(|commit| commit.subject == subject)
        .unwrap()
}

fn refs(commit: &GitLogCommit) -> Vec<(&str, GitRefKind)> {
    commit
        .refs
        .iter()
        .map(|reference| (reference.name.as_str(), reference.kind))
        .collect()
}

fn subjects(log: &GitLog) -> Vec<&str> {
    log.commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect()
}

#[tokio::test]
async fn lists_branches_with_what_they_track_and_where_they_are_checked_out() {
    let fixture = Fixture::new();
    let app = &fixture.app;
    git(app, &["push", "-q", "origin", "main:other"]);
    git(app, &["remote", "set-head", "origin", "main"]);
    git(app, &["branch", "feature"]);
    let linked = format!("{}/linked", fixture.base);
    git(app, &["worktree", "add", "-q", &linked, "feature"]);
    git(app, &["branch", "--track", "orphaned", "origin/other"]);
    git(app, &["push", "-q", "origin", "--delete", "other"]);
    git(app, &["push", "-q", "origin", "main:review"]);
    write(app, "mine.txt", "mine\n");
    commit_all_at(app, "Mine", 2_000);
    fixture.teammate_pushes("theirs.txt");

    let listed = fixture.branches().await;

    assert_eq!(listed.current.as_deref(), Some("main"));
    assert!(!listed.detached);
    assert_eq!(listed.head, Some(fixture.id("HEAD")));
    assert!(!listed.truncated);
    let names: Vec<&str> = listed
        .branches
        .iter()
        .map(|branch| branch.name.as_str())
        .collect();
    assert_eq!(names, ["main", "feature", "orphaned", "origin/review"]);

    let main = named(&listed.branches, "main");
    assert_eq!(main.kind, GitBranchKind::Local);
    assert_eq!(main.tip, fixture.id("main"));
    assert_eq!(main.time, 2_000_000);
    assert_eq!(main.subject, "Mine");
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (Some(1), Some(1)));
    assert_eq!(main.worktree, None);

    let feature = named(&listed.branches, "feature");
    assert_eq!(feature.upstream, None);
    assert_eq!((feature.ahead, feature.behind), (None, None));
    assert_eq!(feature.worktree.as_deref(), Some(linked.as_str()));

    let orphaned = named(&listed.branches, "orphaned");
    assert_eq!(orphaned.upstream, None);
    assert_eq!(orphaned.ahead, None);

    let review = named(&listed.branches, "origin/review");
    assert_eq!(review.kind, GitBranchKind::Remote);
    assert_eq!(review.tip, fixture.id("origin/review"));
    assert_eq!(review.subject, "First");
    assert_eq!(review.time, 1_000_000);
    assert_eq!(
        (
            &review.upstream,
            review.ahead,
            review.behind,
            &review.worktree
        ),
        (&None, None, None, &None)
    );

    let from_linked = branches(Path::new(&linked), &fixture.git).await.unwrap();
    assert_eq!(from_linked.current.as_deref(), Some("feature"));
    assert_eq!(named(&from_linked.branches, "feature").worktree, None);
    assert_eq!(
        named(&from_linked.branches, "main").worktree.as_deref(),
        Some(app.as_str())
    );
}

#[tokio::test]
async fn says_where_head_is_when_it_is_detached_and_before_the_first_commit() {
    let fixture = Fixture::new();
    git(&fixture.app, &["checkout", "-q", "--detach"]);
    let unborn = format!("{}/unborn", fixture.base);
    fs::create_dir_all(&unborn).unwrap();
    git(&unborn, &["init", "-q", "-b", "main"]);

    let detached = fixture.branches().await;
    let empty = branches(Path::new(&unborn), &fixture.git).await.unwrap();
    let history = log(Path::new(&unborn), LOG_DEFAULT, 0, &fixture.git)
        .await
        .unwrap();

    assert_eq!(detached.current, None);
    assert!(detached.detached);
    assert_eq!(detached.head, Some(fixture.id("HEAD")));
    assert_eq!(named(&detached.branches, "main").worktree, None);
    assert_eq!(empty.current.as_deref(), Some("main"));
    assert!(!empty.detached);
    assert_eq!(empty.head, None);
    assert!(empty.branches.is_empty());
    assert!(history.commits.is_empty());
    assert!(!history.has_more);
    assert_eq!((history.head, history.upstream), (None, None));
}

#[tokio::test]
async fn switches_to_a_local_a_remote_and_a_new_branch() {
    let fixture = Fixture::new();
    let app = &fixture.app;
    git(app, &["branch", "feature"]);
    git(app, &["checkout", "-q", "-b", "elsewhere"]);
    write(app, "other.txt", "other\n");
    commit_all(app, "Other");
    git(app, &["push", "-q", "origin", "elsewhere:team/other"]);
    git(app, &["checkout", "-q", "main"]);
    git(app, &["branch", "-q", "-D", "elsewhere"]);
    write(app, "notes.txt", "kept\n");

    let local = fixture.switch("feature", GitSwitchKind::Local).await;
    assert_eq!(
        local,
        Ok(GitSwitch {
            current: Some("feature".to_owned()),
            head: Some(fixture.id("feature")),
        })
    );

    let remote = fixture
        .switch("origin/team/other", GitSwitchKind::Remote)
        .await
        .unwrap();
    assert_eq!(remote.current.as_deref(), Some("team/other"));
    assert_eq!(remote.head, Some(fixture.id("origin/team/other")));
    assert_eq!(
        git(app, &["rev-parse", "--abbrev-ref", "team/other@{upstream}"]),
        "origin/team/other"
    );

    let new = fixture.switch("topic", GitSwitchKind::New).await.unwrap();
    assert_eq!(new.current.as_deref(), Some("topic"));
    assert_eq!(new.head, remote.head);
    assert_eq!(git(app, &["symbolic-ref", "HEAD"]), "refs/heads/topic");
    assert_eq!(
        fs::read_to_string(format!("{app}/notes.txt")).unwrap(),
        "kept\n"
    );
}

#[tokio::test]
async fn leaves_everything_as_it_is_when_git_refuses_a_switch() {
    let fixture = Fixture::new();
    let app = &fixture.app;
    git(app, &["checkout", "-q", "-b", "feature"]);
    write(app, "README.md", "theirs\n");
    commit_all(app, "Feature");
    git(app, &["checkout", "-q", "main"]);
    git(app, &["branch", "taken"]);
    let linked = format!("{}/linked", fixture.base);
    git(app, &["worktree", "add", "-q", &linked, "taken"]);
    write(app, "README.md", "mine\n");

    let clash = fixture
        .switch("feature", GitSwitchKind::Local)
        .await
        .unwrap_err();
    let elsewhere = fixture
        .switch("taken", GitSwitchKind::Local)
        .await
        .unwrap_err();
    let existing = fixture
        .switch("feature", GitSwitchKind::New)
        .await
        .unwrap_err();
    let missing = fixture
        .switch("origin/never", GitSwitchKind::Remote)
        .await
        .unwrap_err();

    assert_eq!(clash.code, ErrorCode::GitFailed);
    assert!(clash.message.contains("would be overwritten"));
    assert!(clash.message.contains("README.md"));
    assert_eq!(elsewhere.code, ErrorCode::GitFailed);
    assert!(elsewhere.message.contains(&linked));
    assert_eq!(existing.code, ErrorCode::GitFailed);
    assert!(existing.message.contains("already exists"));
    assert_eq!(missing.code, ErrorCode::GitFailed);
    for name in [
        "",
        "-",
        "-f",
        "--detach",
        "--orphan=x",
        "@{-1}",
        "a..b",
        "a b",
        "HEAD",
        "ends.lock",
        "line\nbreak",
    ] {
        for kind in [
            GitSwitchKind::Local,
            GitSwitchKind::Remote,
            GitSwitchKind::New,
        ] {
            assert_eq!(
                code(fixture.switch(name, kind).await),
                ErrorCode::BadRequest,
                "{name:?}"
            );
        }
    }
    assert_eq!(git(app, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(
        fs::read_to_string(format!("{app}/README.md")).unwrap(),
        "mine\n"
    );
    assert_eq!(
        git(
            app,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"]
        ),
        "feature\nmain\ntaken"
    );
}

/// A history with a merge in it, a commit that is not pushed and one that is not pulled.
fn history(fixture: &Fixture) {
    let app = &fixture.app;
    git(app, &["tag", "light"]);
    git(app, &["checkout", "-q", "-b", "side"]);
    write(app, "side.txt", "side\n");
    commit_all_at(app, "Side", 2_000);
    git(app, &["checkout", "-q", "main"]);
    write(app, "main.txt", "main\n");
    commit_all_at(app, "Main", 3_000);
    git_at(
        app,
        &["merge", "-q", "--no-ff", "-m", "Merge", "side"],
        4_000,
    );
    git_at(app, &["tag", "-a", "-m", "Release", "v1"], 4_000);
    git(app, &["push", "-q"]);
    write(app, "mine.txt", "mine\n");
    commit_all_at(app, "Mine", 5_000);
    fixture.teammate_pushes("theirs.txt");
}

#[tokio::test]
async fn lists_the_history_of_head_and_its_upstream_with_refs_and_sides() {
    let fixture = Fixture::new();
    history(&fixture);

    let log = fixture.log(LOG_DEFAULT, 0).await;

    assert_eq!(log.commits.len(), 6);
    assert!(!log.has_more);
    assert_eq!(log.head, Some(fixture.id("HEAD")));
    assert_eq!(log.upstream, Some(fixture.id("origin/main")));
    for (index, commit) in log.commits.iter().enumerate() {
        for parent in &commit.parents {
            let parent = log.commits.iter().position(|other| other.id == *parent);
            assert!(parent.unwrap() > index, "{} before its child", commit.id);
        }
    }

    let mine = commit(&log, "Mine");
    assert_eq!(mine.id, fixture.id("HEAD"));
    assert_eq!(mine.side, GitSide::Local);
    assert_eq!(refs(mine), [("main", GitRefKind::Head)]);
    assert_eq!(mine.author, "Test");
    assert_eq!(mine.time, 5_000_000);
    assert_eq!(mine.parents, [fixture.id("HEAD^")]);

    let theirs = commit(&log, "Theirs");
    assert_eq!(theirs.side, GitSide::Remote);
    assert_eq!(refs(theirs), [("origin/main", GitRefKind::Remote)]);

    let merge = commit(&log, "Merge");
    assert_eq!(merge.side, GitSide::Both);
    assert_eq!(merge.parents, [fixture.id("HEAD^^1"), fixture.id("side")]);
    assert_eq!(refs(merge), [("v1", GitRefKind::Tag)]);
    assert_eq!(refs(commit(&log, "Side")), [("side", GitRefKind::Local)]);
    assert_eq!(commit(&log, "Main").side, GitSide::Both);
    let first = commit(&log, "First");
    assert_eq!(refs(first), [("light", GitRefKind::Tag)]);
    assert!(first.parents.is_empty());
    assert_eq!(log.commits.last(), Some(first));
}

#[tokio::test]
async fn pages_through_the_history() {
    let fixture = Fixture::new();
    history(&fixture);
    let whole = fixture.log(LOG_LIMIT, 0).await;

    let start = fixture.log(4, 0).await;
    let end = fixture.log(4, 4).await;
    let exact = fixture.log(2, 4).await;
    let past = fixture.log(4, 60).await;

    assert!(start.has_more);
    assert!(!end.has_more);
    assert_eq!(end.commits.len(), 2);
    assert!(!exact.has_more);
    assert_eq!(
        [subjects(&start), subjects(&end)].concat(),
        subjects(&whole)
    );
    assert_eq!(exact.commits, end.commits);
    assert!(past.commits.is_empty());
    assert!(!past.has_more);
    assert_eq!(past.head, whole.head);
}

#[tokio::test]
async fn lists_the_history_of_a_detached_head_and_of_a_branch_that_tracks_nothing() {
    let fixture = Fixture::new();
    history(&fixture);
    let app = &fixture.app;
    git(app, &["checkout", "-q", "--detach", "side"]);

    let detached = fixture.log(LOG_DEFAULT, 0).await;
    git(app, &["checkout", "-q", "-b", "untracked"]);
    let untracked = fixture.log(LOG_DEFAULT, 0).await;

    assert_eq!(subjects(&detached), ["Side", "First"]);
    assert_eq!(detached.upstream, None);
    assert_eq!(
        refs(&detached.commits[0]),
        [("HEAD", GitRefKind::Head), ("side", GitRefKind::Local)]
    );
    assert!(
        detached
            .commits
            .iter()
            .all(|commit| commit.side == GitSide::Both)
    );
    assert_eq!(
        refs(&untracked.commits[0]),
        [("untracked", GitRefKind::Head), ("side", GitRefKind::Local)]
    );
    assert_eq!(untracked.upstream, None);
}

#[tokio::test]
async fn shows_a_commit_with_its_message_and_the_files_it_changed() {
    let fixture = Fixture::new();
    let app = &fixture.app;
    let root = fixture.id("HEAD");
    write(app, "keep.txt", "one\ntwo\n");
    write(app, "gone.txt", "bye\n");
    write(app, "old.txt", "a\nb\nc\n");
    write(app, "image.bin", [0, 1, 2, 3]);
    commit_all_at(app, "Files", 2_000);
    write(app, "keep.txt", "one\n2\nthree\n");
    fs::remove_file(format!("{app}/gone.txt")).unwrap();
    git(app, &["mv", "old.txt", "new.txt"]);
    write(app, "added.txt", "new\n");
    write(app, "image.bin", [0, 1, 2, 3, 4, 5]);
    let subject = "s".repeat(250);
    commit_all_at(
        app,
        &format!("{subject}\n\nWhy it changed.\n\nAnd more.\n"),
        3_000,
    );
    let changed = fixture.id("HEAD");
    git(app, &["checkout", "-q", "-b", "side", &root]);
    write(app, "side.txt", "side\n");
    commit_all_at(app, "Side", 4_000);
    git(app, &["checkout", "-q", "main"]);
    git_at(
        app,
        &["merge", "-q", "--no-ff", "-m", "Merge", "side"],
        5_000,
    );

    let shown = fixture.show(&changed).await.unwrap();
    let merge = fixture.show(&fixture.id("HEAD")).await.unwrap();
    let first = fixture.show(&root[..7]).await.unwrap();

    assert_eq!(shown.commit.id, changed);
    assert_eq!(shown.commit.subject, "s".repeat(200));
    assert_eq!(shown.commit.time, 3_000_000);
    assert_eq!(shown.commit.side, GitSide::Local);
    assert_eq!(shown.body, "Why it changed.\n\nAnd more.");
    assert!(!shown.truncated);
    let files: Vec<_> = shown
        .files
        .iter()
        .map(|file| {
            (
                file.path.as_str(),
                file.old_path.as_deref(),
                file.status,
                file.additions,
                file.deletions,
                file.binary,
            )
        })
        .collect();
    assert_eq!(
        files,
        [
            (
                "added.txt",
                None,
                GitFileStatus::Added,
                Some(1),
                Some(0),
                false
            ),
            (
                "gone.txt",
                None,
                GitFileStatus::Deleted,
                Some(0),
                Some(1),
                false
            ),
            ("image.bin", None, GitFileStatus::Modified, None, None, true),
            (
                "keep.txt",
                None,
                GitFileStatus::Modified,
                Some(2),
                Some(1),
                false
            ),
            (
                "new.txt",
                Some("old.txt"),
                GitFileStatus::Renamed,
                Some(0),
                Some(0),
                false
            ),
        ]
    );

    assert_eq!(merge.commit.parents.len(), 2);
    assert_eq!(refs(&merge.commit), [("main", GitRefKind::Head)]);
    assert_eq!(merge.body, "");
    let brought: Vec<&str> = merge.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(brought, ["side.txt"]);

    assert_eq!(first.commit.id, root);
    assert_eq!(first.commit.side, GitSide::Both);
    assert!(first.commit.parents.is_empty());
    assert_eq!(first.files.len(), 1);
    assert_eq!(first.files[0].path, "README.md");
    assert_eq!(first.files[0].status, GitFileStatus::Added);
    assert_eq!(first.files[0].additions, Some(1));

    for malformed in ["", "abc", "main", "HEAD", "-n1", "12345g7", &"a".repeat(65)] {
        assert_eq!(
            code(fixture.show(malformed).await),
            ErrorCode::BadRequest,
            "{malformed:?}"
        );
    }
    assert_eq!(
        code(fixture.show("0123456789abcdef").await),
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn answers_what_a_commit_changed_in_one_file() {
    let fixture = Fixture::new();
    let app = &fixture.app;
    let root = fixture.id("HEAD");
    write(app, "keep.txt", "one\ntwo\n");
    write(app, "old.txt", "a\nb\nc\n");
    write(app, "image.bin", [0, 1, 2, 3]);
    fs::create_dir(format!("{app}/directory")).unwrap();
    write(app, "directory/inside.txt", "inside\n");
    commit_all(app, "Files");
    let files = fixture.id("HEAD");
    write(app, "keep.txt", "one\n2\nthree\n");
    git(app, &["mv", "old.txt", "new.txt"]);
    write(app, "image.bin", [0, 1, 2, 3, 4, 5]);
    write(app, "huge.txt", "line\n".repeat(DIFF_LIMIT / 5 + 1));
    commit_all(app, "Changes");
    let changes = fixture.id("HEAD");
    write(app, "keep.txt", "not committed\n");

    let modified = text(fixture.diff(&changes, "keep.txt", None).await);
    let renamed = text(fixture.diff(&changes, "new.txt", Some("old.txt")).await);
    let added = text(fixture.diff(&files, "keep.txt", None).await);
    let in_root = text(fixture.diff(&root[..8], "README.md", None).await);

    assert!(modified.starts_with("diff --git a/keep.txt b/keep.txt\n"));
    assert!(modified.ends_with("@@ -1,2 +1,3 @@\n one\n-two\n+2\n+three\n"));
    assert!(renamed.contains("rename from old.txt\nrename to new.txt\n"));
    assert!(!renamed.contains("@@"));
    assert!(added.ends_with("--- /dev/null\n+++ b/keep.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"));
    assert!(in_root.ends_with("--- /dev/null\n+++ b/README.md\n@@ -0,0 +1 @@\n+first\n"));
    assert_eq!(text(fixture.diff(&changes, "README.md", None).await), "");
    assert_eq!(
        fixture.diff(&changes, "image.bin", None).await,
        Ok(GitDiff::Binary {
            old_size: Some(4),
            new_size: Some(6)
        })
    );
    assert_eq!(
        fixture.diff(&files, "image.bin", None).await,
        Ok(GitDiff::Binary {
            old_size: None,
            new_size: Some(4)
        })
    );
    assert_eq!(
        fixture.diff(&changes, "huge.txt", None).await,
        Ok(GitDiff::TooLarge {
            limit: DIFF_LIMIT as u64
        })
    );
    assert_eq!(
        code(fixture.diff(&changes, "never.txt", None).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(fixture.diff(&root, "keep.txt", None).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(fixture.diff(&changes, "directory", None).await),
        ErrorCode::Failed
    );
    assert_eq!(
        code(fixture.diff(&changes, "../app/keep.txt", None).await),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(fixture.diff("nothex", "keep.txt", None).await),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(fixture.diff("0123456789abcdef", "keep.txt", None).await),
        ErrorCode::NotFound
    );
}
