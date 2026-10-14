use tempfile::TempDir;

use super::*;
use crate::git_run::COMMAND_LIMIT;
use crate::git_test_support::{commit_all, environment, git, identify, write};

/// A repository `app` on `main`, with a branch `feature` that changes `shared.txt` and a bare
/// remote that has both, in a directory that goes away with this.
struct Fixture {
    _directory: TempDir,
    app: String,
    git: Git,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(directory.path()).unwrap();
        let base = base.to_str().unwrap().to_owned();
        let (app, remote) = (format!("{base}/app"), format!("{base}/app.git"));
        fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        identify(&app);
        write(&app, "shared.txt", "one\ntwo\nthree\n");
        commit_all(&app, "First");
        git(&app, &["switch", "-q", "-c", "feature"]);
        write(&app, "shared.txt", "one\nTWO from feature\nthree\n");
        commit_all(&app, "Feature");
        git(&app, &["switch", "-q", "main"]);
        git(&base, &["init", "-q", "--bare", "-b", "main", &remote]);
        git(&app, &["remote", "add", "origin", &remote]);
        git(&app, &["push", "-q", "origin", "main", "feature"]);
        git(&app, &["fetch", "-q"]);
        Self {
            _directory: directory,
            app,
            git: Git::new(None, &environment(), COMMAND_LIMIT),
        }
    }

    fn app(&self) -> &Path {
        Path::new(&self.app)
    }

    fn id(&self, revision: &str) -> String {
        git(&self.app, &["rev-parse", revision])
    }

    fn is_merging(&self) -> bool {
        Path::new(&self.app).join(".git/MERGE_HEAD").exists()
    }

    async fn merge(&self, branch: &str, kind: GitMergeKind) -> Result<GitMerge, RpcError> {
        merge(self.app(), branch, kind, &self.git).await
    }

    /// Commits a change to `shared.txt` on `main` that conflicts with `feature`.
    fn diverge(&self) {
        write(&self.app, "shared.txt", "one\nTWO from main\nthree\n");
        commit_all(&self.app, "Main");
    }
}

#[tokio::test]
async fn fast_forwards_and_then_is_up_to_date() {
    let fixture = Fixture::new();

    let merged = fixture.merge("feature", GitMergeKind::Local).await.unwrap();
    assert_eq!(merged.outcome, GitMergeOutcome::FastForward);
    assert_eq!(merged.head, fixture.id("feature"));
    assert!(merged.conflicts.is_empty());
    assert!(merged.output.contains("Fast-forward"));

    let again = fixture.merge("feature", GitMergeKind::Local).await.unwrap();
    assert_eq!(again.outcome, GitMergeOutcome::UpToDate);
    assert_eq!(again.head, fixture.id("feature"));
}

#[tokio::test]
async fn makes_a_merge_commit_with_the_message_git_makes_for_the_short_name() {
    let fixture = Fixture::new();
    write(&fixture.app, "other.txt", "main's own file\n");
    commit_all(&fixture.app, "Main");

    let merged = fixture.merge("feature", GitMergeKind::Local).await.unwrap();

    assert_eq!(merged.outcome, GitMergeOutcome::Merged);
    assert_eq!(merged.head, fixture.id("HEAD"));
    assert_eq!(fixture.id("HEAD^2"), fixture.id("feature"));
    assert_eq!(
        git(&fixture.app, &["log", "-1", "--format=%B"]),
        "Merge branch 'feature'"
    );
}

#[tokio::test]
async fn merges_a_remote_tracking_branch() {
    let fixture = Fixture::new();
    git(&fixture.app, &["switch", "-q", "-c", "topic"]);
    write(&fixture.app, "other.txt", "topic's own file\n");
    commit_all(&fixture.app, "Topic");

    let merged = fixture
        .merge("origin/feature", GitMergeKind::Remote)
        .await
        .unwrap();

    assert_eq!(merged.outcome, GitMergeOutcome::Merged);
    assert_eq!(
        git(&fixture.app, &["log", "-1", "--format=%B"]),
        "Merge remote-tracking branch 'origin/feature' into topic"
    );
    let missing = fixture.merge("feature", GitMergeKind::Remote).await;
    assert_eq!(missing.unwrap_err().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn stops_at_conflicts_and_leaves_the_merge_unfinished() {
    let fixture = Fixture::new();
    fixture.diverge();
    let before = fixture.id("HEAD");

    let merged = fixture.merge("feature", GitMergeKind::Local).await.unwrap();

    assert_eq!(merged.outcome, GitMergeOutcome::Conflicts);
    assert_eq!(merged.head, before);
    assert_eq!(merged.conflicts, ["shared.txt"]);
    assert!(merged.output.contains("CONFLICT"));
    assert!(fixture.is_merging());
    let state = merge_state(fixture.app(), &fixture.git)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.branch.as_deref(), Some("feature"));
    assert_eq!(state.message, "Merge branch 'feature'");
    assert_eq!(state.conflicts, ["shared.txt"]);

    let again = fixture.merge("feature", GitMergeKind::Local).await;
    assert_eq!(again.unwrap_err().code, ErrorCode::GitFailed);
    assert!(fixture.is_merging());
}

#[tokio::test]
async fn refuses_when_local_changes_would_be_overwritten() {
    let fixture = Fixture::new();
    write(&fixture.app, "shared.txt", "an edit that is not committed\n");
    let before = fixture.id("HEAD");

    let refused = fixture
        .merge("feature", GitMergeKind::Local)
        .await
        .unwrap_err();

    assert_eq!(refused.code, ErrorCode::GitFailed);
    assert!(refused.message.contains("would be overwritten"));
    assert_eq!(fixture.id("HEAD"), before);
    assert!(!fixture.is_merging());
    assert_eq!(
        fs::read_to_string(fixture.app().join("shared.txt")).unwrap(),
        "an edit that is not committed\n"
    );
}

#[tokio::test]
async fn refuses_the_current_branch_a_detached_head_and_unknown_branches() {
    let fixture = Fixture::new();
    let code = |result: Result<GitMerge, RpcError>| result.unwrap_err().code;

    assert_eq!(
        code(fixture.merge("main", GitMergeKind::Local).await),
        ErrorCode::BadRequest
    );
    for unknown in ["nope", "-nope", "feature~1", "feature^{tree}", "HEAD"] {
        assert_eq!(
            code(fixture.merge(unknown, GitMergeKind::Local).await),
            ErrorCode::NotFound,
            "{unknown}"
        );
    }
    assert_eq!(
        code(fixture.merge("", GitMergeKind::Local).await),
        ErrorCode::BadRequest
    );

    git(&fixture.app, &["switch", "-q", "--detach", "main"]);
    assert_eq!(
        code(fixture.merge("feature", GitMergeKind::Local).await),
        ErrorCode::BadRequest
    );
}

#[tokio::test]
async fn merges_a_branch_whose_name_starts_with_a_dash_as_a_branch() {
    let fixture = Fixture::new();
    let feature = fixture.id("feature");
    git(&fixture.app, &["update-ref", "refs/heads/-p", &feature]);

    let merged = fixture.merge("-p", GitMergeKind::Local).await.unwrap();

    assert_eq!(merged.outcome, GitMergeOutcome::FastForward);
    assert_eq!(merged.head, feature);
}

#[tokio::test]
async fn aborts_a_merge_only_while_there_is_one() {
    let fixture = Fixture::new();
    fixture.diverge();
    let before = fixture.id("HEAD");
    let nothing = merge_abort(fixture.app(), &fixture.git).await;
    assert_eq!(nothing.unwrap_err().code, ErrorCode::BadRequest);
    fixture.merge("feature", GitMergeKind::Local).await.unwrap();

    let aborted = merge_abort(fixture.app(), &fixture.git).await.unwrap();

    assert_eq!(aborted.head, before);
    assert!(!fixture.is_merging());
    assert_eq!(git(&fixture.app, &["status", "--porcelain"]), "");
    assert_eq!(merge_state(fixture.app(), &fixture.git).await.unwrap(), None);
}

#[tokio::test]
async fn refuses_to_commit_a_merge_while_markers_remain() {
    let fixture = Fixture::new();
    assert_eq!(check_conflict_markers(fixture.app(), &fixture.git).await, Ok(()));
    fixture.diverge();
    fixture.merge("feature", GitMergeKind::Local).await.unwrap();

    let refused = check_conflict_markers(fixture.app(), &fixture.git)
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.message, "Conflict markers remain in:\nshared.txt");

    write(&fixture.app, "shared.txt", "one\nTWO from both\nthree\n");
    assert_eq!(check_conflict_markers(fixture.app(), &fixture.git).await, Ok(()));
}

#[test]
fn finds_the_lines_git_marks_conflicts_with() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("file.txt");
    let marked = |content: &str| {
        fs::write(&file, content).unwrap();
        has_conflict_marker(&file)
    };
    assert!(marked("a\n<<<<<<< HEAD\nb\n"));
    assert!(marked("a\r\n=======\r\nb\r\n"));
    assert!(marked(">>>>>>> feature"));
    assert!(!marked("a\n<<<<<<<\n======== \n >>>>>>> x\n=======x\n"));
    assert!(!has_conflict_marker(&directory.path().join("missing.txt")));
}

#[test]
fn names_at_most_twenty_paths() {
    let paths: Vec<String> = (1..=23).map(|n| format!("file{n}")).collect();
    let message = markers_remain(&paths);
    assert!(message.starts_with("Conflict markers remain in:\nfile1\n"));
    assert!(message.contains("\nfile20\nand 3 more"));
    assert!(!message.contains("file21"));
}

#[test]
fn reads_the_branch_and_message_git_prepared() {
    let prepared = "Merge branch 'feature' into main\n\n# Conflicts:\n#\tshared.txt\n";
    assert_eq!(merged_branch(prepared).as_deref(), Some("feature"));
    assert_eq!(without_comments(prepared), "Merge branch 'feature' into main");
    assert_eq!(
        merged_branch("Merge remote-tracking branch 'origin/x'").as_deref(),
        Some("origin/x")
    );
    assert_eq!(
        merged_branch("Merge branch 'refs/heads/x'").as_deref(),
        Some("x")
    );
    assert_eq!(merged_branch("Something else"), None);
    assert_eq!(merged_branch(""), None);
}
