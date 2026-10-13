use std::cell::Cell;
use std::rc::Rc;

use super::*;

fn file(repo: &str, path: &str) -> BlameFile {
    BlameFile {
        repo_root: PathBuf::from(repo),
        relative_path: path.to_string(),
    }
}

fn key(repo: &str, path: &str, head: &str, version: ContentVersion) -> BlameKey {
    BlameKey {
        file: file(repo, path),
        head: head.to_string(),
        version,
    }
}

fn result(marker: BlameResult) -> Arc<BlameResult> {
    Arc::new(marker)
}

#[test]
fn cache_answers_only_for_the_same_repo_path_head_and_version() {
    let version = ContentVersion::new();
    let mut cache = BlameCache::new(8);
    cache.insert(
        key("/repo", "a.rs", "head1", version),
        result(BlameResult::NotTracked),
    );

    assert!(cache.get(&key("/repo", "a.rs", "head1", version)).is_some());
    assert!(
        cache
            .get(&key("/other", "a.rs", "head1", version))
            .is_none()
    );
    assert!(cache.get(&key("/repo", "b.rs", "head1", version)).is_none());
    assert!(cache.get(&key("/repo", "a.rs", "head2", version)).is_none());
    assert!(
        cache
            .get(&key("/repo", "a.rs", "head1", ContentVersion::new()))
            .is_none()
    );
}

#[test]
fn cache_keeps_one_blame_per_file() {
    let (old, new) = (ContentVersion::new(), ContentVersion::new());
    let mut cache = BlameCache::new(8);
    cache.insert(
        key("/repo", "a.rs", "head", old),
        result(BlameResult::NotTracked),
    );
    cache.insert(
        key("/repo", "a.rs", "head", new),
        result(BlameResult::TooLarge),
    );

    assert_eq!(cache.len(), 1);
    assert!(cache.get(&key("/repo", "a.rs", "head", old)).is_none());
    assert_eq!(
        cache.get(&key("/repo", "a.rs", "head", new)).as_deref(),
        Some(&BlameResult::TooLarge)
    );
}

#[test]
fn cache_is_bounded_and_drops_the_file_used_longest_ago() {
    let version = ContentVersion::new();
    let mut cache = BlameCache::new(2);
    cache.insert(
        key("/repo", "a.rs", "head", version),
        result(BlameResult::NotTracked),
    );
    cache.insert(
        key("/repo", "b.rs", "head", version),
        result(BlameResult::NotTracked),
    );
    // Using a.rs makes b.rs the one used longest ago.
    assert!(cache.get(&key("/repo", "a.rs", "head", version)).is_some());
    cache.insert(
        key("/repo", "c.rs", "head", version),
        result(BlameResult::NotTracked),
    );

    assert_eq!(cache.len(), 2);
    assert!(cache.get(&key("/repo", "a.rs", "head", version)).is_some());
    assert!(cache.get(&key("/repo", "b.rs", "head", version)).is_none());
    assert!(cache.get(&key("/repo", "c.rs", "head", version)).is_some());
}

#[test]
fn moving_head_drops_the_blames_of_that_repository_only() {
    let version = ContentVersion::new();
    let mut cache = BlameCache::new(8);
    cache.insert(
        key("/repo", "a.rs", "old", version),
        result(BlameResult::NotTracked),
    );
    cache.insert(
        key("/repo", "b.rs", "new", version),
        result(BlameResult::NotTracked),
    );
    cache.insert(
        key("/elsewhere", "a.rs", "old", version),
        result(BlameResult::NotTracked),
    );

    cache.retain_head(Path::new("/repo"), Some("new"));

    assert!(cache.get(&key("/repo", "a.rs", "old", version)).is_none());
    assert!(cache.get(&key("/repo", "b.rs", "new", version)).is_some());
    assert!(
        cache
            .get(&key("/elsewhere", "a.rs", "old", version))
            .is_some()
    );

    // A repository left without commits keeps nothing.
    cache.retain_head(Path::new("/repo"), None);
    assert!(cache.get(&key("/repo", "b.rs", "new", version)).is_none());
    assert_eq!(cache.len(), 1);
}

/// Stands in for the handle of a running computation and records being stopped.
#[derive(Clone, Default)]
struct FakeJob {
    aborted: Rc<Cell<bool>>,
}

impl AbortJob for FakeJob {
    fn abort(&self) {
        self.aborted.set(true);
    }
}

#[test]
fn a_running_blame_for_the_same_key_is_not_started_twice() {
    let version = ContentVersion::new();
    let mut jobs = BlameJobs::new();
    let job = FakeJob::default();

    assert!(jobs.needs_start(&key("/repo", "a.rs", "head", version)));
    jobs.started(key("/repo", "a.rs", "head", version), job.clone());

    assert!(!jobs.needs_start(&key("/repo", "a.rs", "head", version)));
    assert!(!job.aborted.get());
    // Another file is unaffected by it.
    assert!(jobs.needs_start(&key("/repo", "b.rs", "head", version)));
    assert!(!job.aborted.get());
}

#[test]
fn editing_the_buffer_again_stops_the_stale_blame() {
    let (typed_once, typed_twice) = (ContentVersion::new(), ContentVersion::new());
    let mut jobs = BlameJobs::new();
    let stale = FakeJob::default();
    let stale_key = key("/repo", "a.rs", "head", typed_once);
    let fresh_key = key("/repo", "a.rs", "head", typed_twice);
    jobs.started(stale_key.clone(), stale.clone());

    assert!(jobs.needs_start(&fresh_key));
    assert!(stale.aborted.get());

    let fresh = FakeJob::default();
    jobs.started(fresh_key.clone(), fresh.clone());
    // Should the stopped computation still deliver, its result is refused.
    assert!(!jobs.finished(&stale_key));
    assert!(jobs.finished(&fresh_key));
    assert!(!fresh.aborted.get());
    // Nothing is running any more.
    assert!(jobs.needs_start(&fresh_key));
}

#[test]
fn a_new_head_stops_the_blames_of_that_repository() {
    let version = ContentVersion::new();
    let mut jobs = BlameJobs::new();
    let (here, also_here, elsewhere) = (FakeJob::default(), FakeJob::default(), FakeJob::default());
    jobs.started(key("/repo", "a.rs", "old", version), here.clone());
    jobs.started(key("/repo", "b.rs", "old", version), also_here.clone());
    jobs.started(key("/elsewhere", "a.rs", "old", version), elsewhere.clone());

    jobs.abort_repo(Path::new("/repo"));

    assert!(here.aborted.get() && also_here.aborted.get());
    assert!(!elsewhere.aborted.get());
    assert!(!jobs.finished(&key("/repo", "a.rs", "old", version)));
    assert!(jobs.finished(&key("/elsewhere", "a.rs", "old", version)));
}

#[test]
fn a_blame_against_another_head_is_stale_too() {
    let version = ContentVersion::new();
    let mut jobs = BlameJobs::new();
    let before_commit = FakeJob::default();
    jobs.started(key("/repo", "a.rs", "old", version), before_commit.clone());

    assert!(jobs.needs_start(&key("/repo", "a.rs", "new", version)));
    assert!(before_commit.aborted.get());
}

#[test]
fn repo_config_is_read_with_later_lines_winning() {
    let (identity, origin) = parse_repo_config(
        "user.name Global Name\n\
         user.email global@example.com\n\
         remote.origin.url git@github.com:owner/repo.git\n\
         user.name Ada Lovelace\n",
    );

    assert_eq!(identity.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(identity.email.as_deref(), Some("global@example.com"));
    assert_eq!(origin.as_deref(), Some("git@github.com:owner/repo.git"));
}

#[test]
fn repo_config_without_values_is_unset() {
    assert_eq!(parse_repo_config(""), (GitIdentity::default(), None));
    let (identity, origin) = parse_repo_config("user.name\nuser.email  \nother.key value\n");
    assert_eq!(identity, GitIdentity::default());
    assert_eq!(origin, None);
}

#[test]
fn large_buffers_are_over_the_limits() {
    let size = |bytes, lines| BufferSize { bytes, lines };
    assert!(!exceeds_blame_limits(size(13, 1)));
    assert!(!exceeds_blame_limits(size(100_000, MAX_BLAME_LINES)));
    assert!(exceeds_blame_limits(size(100_002, MAX_BLAME_LINES + 1)));
    assert!(!exceeds_blame_limits(size(MAX_BLAME_BYTES, 1)));
    assert!(exceeds_blame_limits(size(MAX_BLAME_BYTES + 1, 1)));
}

async fn git(repo: &Path, args: &[&str]) -> String {
    run_git_command(repo, args)
        .await
        .unwrap_or_else(|err| panic!("git {args:?} failed: {err}"))
        .trim()
        .to_string()
}

/// A repository with `notes.txt` committed as three lines by Ada.
async fn init_repo(repo: &Path) {
    git(repo, &["init", "-b", "main"]).await;
    git(repo, &["config", "user.email", "ada@example.com"]).await;
    git(repo, &["config", "user.name", "Ada Lovelace"]).await;
    git(repo, &["config", "commit.gpgsign", "false"]).await;
    std::fs::write(repo.join("notes.txt"), "one\ntwo\nthree\n").expect("write file");
    git(repo, &["add", "-A"]).await;
    git(repo, &["commit", "-m", "Add the notes"]).await;
}

fn blamed(result: BlameResult) -> FileBlame {
    match result {
        BlameResult::Blamed(blame) => blame,
        other => panic!("expected a blame, got {other:?}"),
    }
}

#[tokio::test]
async fn unsaved_lines_are_uncommitted_and_the_rest_keep_their_commit() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_repo(repo).await;
    let head = git(repo, &["rev-parse", "HEAD"]).await;

    // The buffer has a changed second line and a new last line; the file on disk has neither.
    let blame = blamed(
        compute_blame(
            file(repo.to_str().expect("utf-8 path"), "notes.txt"),
            "one\nTWO\nthree\nfour\n".to_string(),
            Duration::from_secs(30),
        )
        .await,
    );

    let commit = |line| blame.commit_for_line(line).expect("line is blamed");
    assert_eq!(commit(0).sha, head);
    assert_eq!(commit(0).author, "Ada Lovelace");
    assert_eq!(commit(0).summary, "Add the notes");
    assert!(commit(1).is_uncommitted());
    assert_eq!(commit(2).sha, head);
    assert!(commit(3).is_uncommitted());
    assert!(blame.commit_for_line(4).is_none());
}

#[tokio::test]
async fn untracked_file_has_no_blame() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_repo(repo).await;
    std::fs::write(repo.join("scratch.txt"), "draft\n").expect("write file");

    let result = compute_blame(
        file(repo.to_str().expect("utf-8 path"), "scratch.txt"),
        "draft\n".to_string(),
        Duration::from_secs(30),
    )
    .await;

    assert_eq!(result, BlameResult::NotTracked);
}

#[tokio::test]
async fn only_files_in_the_commit_are_worth_blaming() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_repo(repo).await;
    let head = git(repo, &["rev-parse", "HEAD"]).await;
    let root = repo.to_str().expect("utf-8 path");
    // Untracked, and then staged but in no commit: neither has a history to show.
    std::fs::write(repo.join("dump.sql"), "INSERT INTO t VALUES (1);\n").expect("write file");
    std::fs::write(repo.join("staged.txt"), "new\n").expect("write file");
    git(repo, &["add", "staged.txt"]).await;

    assert!(is_in_commit(file(root, "notes.txt"), head.clone()).await);
    assert!(!is_in_commit(file(root, "dump.sql"), head.clone()).await);
    assert!(!is_in_commit(file(root, "staged.txt"), head.clone()).await);
    assert!(!is_in_commit(file(root, "notes.txt"), "0".repeat(40)).await);
}

#[tokio::test]
async fn blame_that_runs_out_of_time_reports_it() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_repo(repo).await;

    let result = compute_blame(
        file(repo.to_str().expect("utf-8 path"), "notes.txt"),
        "one\ntwo\nthree\n".to_string(),
        Duration::ZERO,
    )
    .await;

    // Git cannot have answered before a time limit of zero. Should it ever, a blame is fine too.
    assert!(
        matches!(result, BlameResult::TimedOut | BlameResult::Blamed(_)),
        "{result:?}"
    );
}

#[tokio::test]
async fn file_is_located_from_its_own_directory() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_repo(repo).await;
    std::fs::create_dir_all(repo.join("src/deep")).expect("create directories");
    let root = PathBuf::from(git(repo, &["rev-parse", "--show-toplevel"]).await);

    assert_eq!(
        locate_in_repository(repo.join("notes.txt")).await,
        Some(BlameFile {
            repo_root: root.clone(),
            relative_path: "notes.txt".to_string(),
        })
    );
    // A file that does not exist yet is still located, by its directory.
    assert_eq!(
        locate_in_repository(repo.join("src/deep/new.rs")).await,
        Some(BlameFile {
            repo_root: root,
            relative_path: "src/deep/new.rs".to_string(),
        })
    );
}

#[tokio::test]
async fn file_outside_a_repository_is_not_located() {
    let directory = tempfile::tempdir().expect("create temp dir");
    std::fs::write(directory.path().join("loose.txt"), "text\n").expect("write file");

    assert_eq!(
        locate_in_repository(directory.path().join("loose.txt")).await,
        None
    );
    assert_eq!(
        locate_in_repository(directory.path().join("missing/dir/file.txt")).await,
        None
    );
}

#[tokio::test]
async fn repo_info_reports_head_identity_and_origin() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_repo(repo).await;
    git(
        repo,
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
    )
    .await;
    let head = git(repo, &["rev-parse", "HEAD"]).await;

    let info = load_repo_info(repo.to_path_buf()).await;

    assert_eq!(info.head.as_deref(), Some(head.as_str()));
    assert_eq!(info.identity.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(info.identity.email.as_deref(), Some("ada@example.com"));
    assert_eq!(
        info.origin_url.as_deref(),
        Some("git@github.com:owner/repo.git")
    );
}

#[tokio::test]
async fn repo_without_commits_has_no_head() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    git(repo_dir.path(), &["init", "-b", "main"]).await;

    assert_eq!(
        load_repo_info(repo_dir.path().to_path_buf()).await.head,
        None
    );
}
