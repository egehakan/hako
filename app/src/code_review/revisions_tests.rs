use std::path::Path;

use warp_util::git::run_git_command;

use super::*;

async fn git(repo: &Path, args: &[&str]) -> String {
    run_git_command(repo, args)
        .await
        .unwrap_or_else(|err| panic!("git {args:?} failed: {err}"))
        .trim()
        .to_string()
}

async fn init_repo(repo: &Path) {
    git(repo, &["init", "-b", "main"]).await;
    git(repo, &["config", "user.email", "test@test.com"]).await;
    git(repo, &["config", "user.name", "Test Author"]).await;
    git(repo, &["config", "commit.gpgsign", "false"]).await;
    git(repo, &["config", "tag.gpgsign", "false"]).await;
}

async fn commit_file(repo: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(repo.join(name), content).expect("write file");
    git(repo, &["add", "-A"]).await;
    git(repo, &["commit", "-m", message]).await;
}

/// A repository with three commits on `main`, a lightweight and an annotated tag, and an
/// `origin` holding `main` and `feature/remote`.
async fn repo_with_history(root: &Path) -> std::path::PathBuf {
    let origin = root.join("origin.git");
    let repo = root.join("repo");
    std::fs::create_dir_all(&origin).expect("create origin dir");
    std::fs::create_dir_all(&repo).expect("create repo dir");
    git(&origin, &["init", "--bare", "-b", "main"]).await;
    init_repo(&repo).await;

    commit_file(&repo, "file.txt", "one\n", "First commit").await;
    commit_file(&repo, "file.txt", "one\ntwo\n", "Second commit").await;
    commit_file(&repo, "file.txt", "one\ntwo\nthree\n", "Fix\ttabs and ünïcode").await;
    git(&repo, &["tag", "v1.0", "HEAD~1"]).await;
    git(&repo, &["tag", "-a", "v2.0", "-m", "Release two"]).await;

    let origin_url = origin.to_str().expect("utf-8 path");
    git(&repo, &["remote", "add", "origin", origin_url]).await;
    git(&repo, &["push", "origin", "main:main", "HEAD~1:refs/heads/feature/remote"]).await;
    repo
}

#[test]
fn parse_ref_listing_reads_remote_branches_and_tags() {
    let output = "refs/remotes/origin/main\tabc1234\t\t3 days ago\tAdd the thing\n\
        refs/remotes/origin/HEAD\tabc1234\t\t3 days ago\tAdd the thing\n\
        refs/remotes/origin/feature/x\tdef5678\t\t2 weeks ago\tSubject\twith a tab\n\
        refs/tags/v1.0\t1111111\t\t5 months ago\tLightweight\n\
        refs/tags/v2.0\t2222222\t3333333\t4 weeks ago\tRelease two\n\
        refs/heads/local\t4444444\t\tnow\tNot listed\n\
        garbage\n";

    let entries = parse_ref_listing(output);
    let summary: Vec<_> = entries
        .iter()
        .map(|entry| (entry.kind, entry.rev.as_str(), entry.short_sha.as_str()))
        .collect();
    assert_eq!(summary, [
        (RevisionKind::RemoteBranch, "origin/main", "abc1234"),
        (RevisionKind::RemoteBranch, "origin/feature/x", "def5678"),
        (RevisionKind::Tag, "v1.0", "1111111"),
        // An annotated tag is listed by the commit it points at.
        (RevisionKind::Tag, "v2.0", "3333333"),
    ]);
    assert_eq!(entries[1].subject, "Subject\twith a tab");
    assert_eq!(entries[1].relative_date, "2 weeks ago");
    assert!(entries.iter().all(|entry| entry.author.is_empty()));
}

#[test]
fn parse_commit_listing_keeps_tabs_and_unicode_in_the_subject() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let output = format!(
        "{sha}\t0123456\tZoë Größe\t2 hours ago\tFix\ttabs and ünïcode\n\
         {sha}\t0123456\tNo Subject\t3 days ago\t\n\
         \n\
         incomplete\tline\n"
    );

    let entries = parse_commit_listing(&output);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], RevisionEntry {
        kind: RevisionKind::Commit,
        rev: sha.to_string(),
        short_sha: "0123456".to_string(),
        subject: "Fix\ttabs and ünïcode".to_string(),
        author: "Zoë Größe".to_string(),
        relative_date: "2 hours ago".to_string(),
    });
    assert_eq!(entries[1].subject, "");
}

#[test]
fn parse_revision_input_accepts_single_revisions() {
    for rev in [
        "main",
        "origin/feature/x",
        "v1.0",
        "HEAD~3",
        "HEAD^2",
        "0123abc",
        "main@{2 days ago}",
        "@{upstream}",
        ":/fix the bug",
    ] {
        assert_eq!(parse_revision_input(rev), Ok(rev), "{rev}");
    }
    assert_eq!(parse_revision_input("  HEAD~3\n"), Ok("HEAD~3"));
}

#[test]
fn parse_revision_input_rejects_what_is_not_one_revision() {
    assert_eq!(parse_revision_input(""), Err(RevisionError::Empty));
    assert_eq!(parse_revision_input("   "), Err(RevisionError::Empty));
    assert_eq!(parse_revision_input("main..feature"), Err(RevisionError::Range));
    assert_eq!(parse_revision_input("main...feature"), Err(RevisionError::Range));
    assert_eq!(parse_revision_input("--all"), Err(RevisionError::Malformed));
    assert_eq!(parse_revision_input("-n1"), Err(RevisionError::Malformed));
    assert_eq!(parse_revision_input("main\nHEAD"), Err(RevisionError::Malformed));
}

#[tokio::test]
async fn resolve_commit_resolves_branches_tags_commits_and_expressions() {
    let root = tempfile::tempdir().expect("create temp dir");
    let repo = repo_with_history(root.path()).await;
    let head = git(&repo, &["rev-parse", "HEAD"]).await;
    let previous = git(&repo, &["rev-parse", "HEAD~1"]).await;

    assert_eq!(resolve_commit(&repo, "main").await, Ok(head.clone()));
    assert_eq!(resolve_commit(&repo, "origin/main").await, Ok(head.clone()));
    assert_eq!(resolve_commit(&repo, "origin/feature/remote").await, Ok(previous.clone()));
    assert_eq!(resolve_commit(&repo, "v1.0").await, Ok(previous.clone()));
    // An annotated tag resolves to its commit, not to the tag object.
    assert_eq!(resolve_commit(&repo, "v2.0").await, Ok(head.clone()));
    assert_eq!(resolve_commit(&repo, "HEAD~1").await, Ok(previous.clone()));
    assert_eq!(resolve_commit(&repo, &previous).await, Ok(previous.clone()));
    assert_eq!(resolve_commit(&repo, &previous[..8]).await, Ok(previous.clone()));
    assert_eq!(resolve_commit(&repo, " HEAD ").await, Ok(head));
    assert_eq!(resolve_commit(&repo, ":/Second commit").await, Ok(previous));
}

#[tokio::test]
async fn resolve_commit_reports_why_a_revision_cannot_be_used() {
    let root = tempfile::tempdir().expect("create temp dir");
    let repo = repo_with_history(root.path()).await;

    assert_eq!(
        resolve_commit(&repo, "no-such-branch").await,
        Err(RevisionError::NotFound("no-such-branch".to_string()))
    );
    assert_eq!(
        resolve_commit(&repo, "HEAD~30").await,
        Err(RevisionError::NotFound("HEAD~30".to_string()))
    );
    assert_eq!(
        resolve_commit(&repo, "HEAD:file.txt").await,
        Err(RevisionError::NotACommit("HEAD:file.txt".to_string()))
    );
    assert_eq!(resolve_commit(&repo, "main..v1.0").await, Err(RevisionError::Range));
    assert_eq!(resolve_commit(&repo, "--all").await, Err(RevisionError::Malformed));
    assert_eq!(resolve_commit(&repo, "").await, Err(RevisionError::Empty));
}

#[tokio::test]
async fn list_revision_candidates_lists_remote_branches_tags_and_commits() {
    let root = tempfile::tempdir().expect("create temp dir");
    let repo = repo_with_history(root.path()).await;
    let head = git(&repo, &["rev-parse", "HEAD"]).await;
    let head_short = git(&repo, &["rev-parse", "--short", "HEAD"]).await;

    let candidates = list_revision_candidates(&repo).await;

    let mut remote_branches: Vec<_> = candidates
        .remote_branches
        .iter()
        .map(|entry| entry.rev.as_str())
        .collect();
    remote_branches.sort_unstable();
    assert_eq!(remote_branches, ["origin/feature/remote", "origin/main"]);
    assert!(
        candidates
            .remote_branches
            .iter()
            .all(|entry| entry.kind == RevisionKind::RemoteBranch && !entry.relative_date.is_empty())
    );

    let annotated = candidates
        .tags
        .iter()
        .find(|entry| entry.rev == "v2.0")
        .expect("annotated tag is listed");
    assert_eq!(annotated.short_sha, head_short);
    assert_eq!(annotated.subject, "Release two");
    assert!(candidates.tags.iter().any(|entry| entry.rev == "v1.0"));
    assert_eq!(candidates.tags.len(), 2);

    let subjects: Vec<_> = candidates
        .commits
        .iter()
        .map(|entry| entry.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Fix\ttabs and ünïcode", "Second commit", "First commit"]);
    let newest = &candidates.commits[0];
    assert_eq!(newest.rev, head);
    assert_eq!(newest.short_sha, head_short);
    assert_eq!(newest.author, "Test Author");
    assert!(!newest.relative_date.is_empty());
}

#[tokio::test]
async fn list_revision_candidates_is_empty_for_a_repository_without_commits() {
    let root = tempfile::tempdir().expect("create temp dir");
    init_repo(root.path()).await;

    assert_eq!(
        list_revision_candidates(root.path()).await,
        RevisionCandidates::default()
    );
}
