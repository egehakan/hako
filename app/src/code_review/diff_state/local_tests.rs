use super::*;
use crate::util::git::{
    BranchEntry, parse_range, parse_unified_diff_header, sort_branches_main_first,
};

#[test]
fn test_parse_range_with_comma() {
    let (start, count) =
        parse_range("10,5").expect("parse_range should succeed for range with count");
    assert_eq!(start, 10);
    assert_eq!(count, 5);
}

#[test]
fn test_parse_range_without_comma() {
    let (start, count) =
        parse_range("10").expect("parse_range should succeed for range without count");
    assert_eq!(start, 10);
    assert_eq!(count, 1);
}

#[test]
fn test_parse_unified_diff_header_basic() {
    let header = "@@ -10,5 +12,7 @@";
    let parsed = parse_unified_diff_header(header)
        .expect("parse_unified_diff_header should succeed for basic header");
    assert_eq!(parsed.old_start_line, 10);
    assert_eq!(parsed.old_line_count, 5);
    assert_eq!(parsed.new_start_line, 12);
    assert_eq!(parsed.new_line_count, 7);
}

#[test]
fn test_parse_unified_diff_header_with_context() {
    let header = "@@ -4978,33 +4978,43 @@ impl TerminalView {";
    let parsed = parse_unified_diff_header(header)
        .expect("parse_unified_diff_header should succeed for header with context");
    assert_eq!(parsed.old_start_line, 4978);
    assert_eq!(parsed.old_line_count, 33);
    assert_eq!(parsed.new_start_line, 4978);
    assert_eq!(parsed.new_line_count, 43);
}

#[test]
fn test_parse_unified_diff_header_single_line() {
    let header = "@@ -10 +12,3 @@";
    let parsed = parse_unified_diff_header(header)
        .expect("parse_unified_diff_header should succeed for single line header");
    assert_eq!(parsed.old_start_line, 10);
    assert_eq!(parsed.old_line_count, 1);
    assert_eq!(parsed.new_start_line, 12);
    assert_eq!(parsed.new_line_count, 3);
}

#[test]
fn test_sort_branches_main_first_empty() {
    let branches: Vec<BranchEntry> = vec![];
    let result: Vec<_> = sort_branches_main_first(&branches).collect();
    assert!(result.is_empty());
}

#[test]
fn test_sort_branches_main_first_no_main() {
    let branches = vec![
        BranchEntry {
            name: "feature-a".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "feature-b".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "feature-c".to_string(),
            is_main: false,
        },
    ];
    let result: Vec<_> = sort_branches_main_first(&branches).collect();
    // No main branches — order should be unchanged.
    assert_eq!(result, branches.iter().collect::<Vec<_>>());
}

#[test]
fn test_sort_branches_main_first_promotes_main() {
    let branches = vec![
        BranchEntry {
            name: "feature-a".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "main".to_string(),
            is_main: true,
        },
        BranchEntry {
            name: "feature-b".to_string(),
            is_main: false,
        },
    ];
    let result: Vec<_> = sort_branches_main_first(&branches)
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(result, vec!["main", "feature-a", "feature-b"]);
}

#[test]
fn test_sort_branches_main_first_main_already_first() {
    let branches = vec![
        BranchEntry {
            name: "main".to_string(),
            is_main: true,
        },
        BranchEntry {
            name: "feature-a".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "feature-b".to_string(),
            is_main: false,
        },
    ];
    let result: Vec<_> = sort_branches_main_first(&branches)
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(result, vec!["main", "feature-a", "feature-b"]);
}

#[test]
fn test_sort_branches_main_first_preserves_recency_order_for_non_main() {
    // Non-main branches should remain in their original (recency) order.
    let branches = vec![
        BranchEntry {
            name: "recent-feature".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "main".to_string(),
            is_main: true,
        },
        BranchEntry {
            name: "older-feature".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "oldest-feature".to_string(),
            is_main: false,
        },
    ];
    let result: Vec<_> = sort_branches_main_first(&branches)
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        result,
        vec!["main", "recent-feature", "older-feature", "oldest-feature"]
    );
}

#[test]
fn test_sort_branches_main_first_multiple_main_flags() {
    // Defensive: both flagged as main (shouldn't happen in practice, but
    // sort_branches_main_first should handle it gracefully).
    let branches = vec![
        BranchEntry {
            name: "feature".to_string(),
            is_main: false,
        },
        BranchEntry {
            name: "main".to_string(),
            is_main: true,
        },
        BranchEntry {
            name: "master".to_string(),
            is_main: true,
        },
    ];
    let result: Vec<_> = sort_branches_main_first(&branches)
        .map(|entry| entry.name.as_str())
        .collect();
    // Both main-flagged entries appear first, non-main last.
    assert_eq!(result, vec!["main", "master", "feature"]);
}

#[test]
fn test_parse_unified_diff_header_malformed() {
    let header = "not a diff header";
    let result = parse_unified_diff_header(header);
    assert!(result.is_err());

    let header2 = "@@ incomplete";
    let result2 = parse_unified_diff_header(header2);
    assert!(result2.is_err());
}

#[test]
fn test_parse_git_status_modified_file_with_spaces() {
    // Porcelain v2 output for a modified file with spaces in the name.
    // Format: 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
    let status_output = "1 .M N... 100644 100644 100644 abc1234 def5678 test file.txt";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "test file.txt");
    assert_eq!(result[0].1, GitFileStatus::Modified);
}

#[test]
fn test_parse_git_status_modified_file_with_multiple_spaces() {
    // Filename with multiple spaces.
    let status_output = "1 .M N... 100644 100644 100644 abc1234 def5678 path to/my test file.txt";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "path to/my test file.txt");
    assert_eq!(result[0].1, GitFileStatus::Modified);
}

#[test]
fn test_parse_git_status_new_file_with_spaces() {
    let status_output = "1 A. N... 000000 100644 100644 0000000 abc1234 new file name.rs";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "new file name.rs");
    assert_eq!(result[0].1, GitFileStatus::New);
}

#[test]
fn test_parse_git_status_renamed_file_with_spaces() {
    // Porcelain v2 renamed entry (type 2) with spaces in the new path.
    // Format: 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>
    let status_output =
        "2 R. N... 100644 100644 100644 abc1234 def5678 R100 new name.txt\0old name.txt";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "new name.txt");
    assert!(matches!(
        &result[0].1,
        GitFileStatus::Renamed { old_path } if old_path == "old name.txt"
    ));
}

#[test]
fn test_parse_git_status_untracked_file_with_spaces() {
    let status_output = "? my untracked file.txt";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "my untracked file.txt");
    assert_eq!(result[0].1, GitFileStatus::Untracked);
}

#[test]
fn test_parse_git_status_unmerged_file_with_spaces() {
    // Porcelain v2 unmerged entry (type u) with spaces in the path.
    // Format: u <xy> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
    let status_output =
        "u UU N... 100644 100644 100644 100644 abc1234 def5678 ghi9012 conflict file.txt";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "conflict file.txt");
    assert_eq!(result[0].1, GitFileStatus::Conflicted);
}

#[test]
fn test_parse_git_status_mixed_entries_with_spaces() {
    // Multiple entries separated by NUL, mixing files with and without spaces.
    let status_output = "1 .M N... 100644 100644 100644 abc1234 def5678 test file.txt\0\
         1 .M N... 100644 100644 100644 abc1234 def5678 normal.txt\0\
         ? another file with spaces.rs";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].0, "test file.txt");
    assert_eq!(result[1].0, "normal.txt");
    assert_eq!(result[2].0, "another file with spaces.rs");
}

#[test]
fn test_parse_git_status_file_without_spaces_still_works() {
    // Ensure the splitn change doesn't break files without spaces.
    let status_output = "1 .M N... 100644 100644 100644 abc1234 def5678 simple.txt";
    let result = LocalDiffStateModel::parse_git_status(status_output).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "simple.txt");
    assert_eq!(result[0].1, GitFileStatus::Modified);
}

#[tokio::test]
async fn untracked_directory_diff_is_empty_and_non_binary() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    std::fs::create_dir(repo_dir.path().join("nested-repo")).expect("create nested dir");

    // `git status` reports a nested repo/worktree as a single untracked
    // directory entry (with a trailing slash). It must short-circuit to an
    // empty non-binary diff — the error fallback would otherwise mislabel it
    // as binary and the view would render "Binary file - no diff available"
    // instead of "New empty file".
    let diff = LocalDiffStateModel::get_file_diff(
        repo_dir.path(),
        "nested-repo/",
        &GitFileStatus::Untracked,
        false,
        None,
    )
    .await
    .expect("get_file_diff should succeed for an untracked directory");

    assert!(!diff.is_binary);
    assert_eq!(diff.hunks.len(), 0);
    assert_eq!(diff.status, GitFileStatus::Untracked);
}

#[tokio::test]
async fn untracked_directory_has_no_baseline_content() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    std::fs::create_dir(repo_dir.path().join("nested-repo")).expect("create nested dir");
    std::fs::write(repo_dir.path().join("new-file.txt"), "hello\n").expect("write file");

    // No baseline for a directory entry, so no editor is constructed for it.
    let dir_content = LocalDiffStateModel::get_file_content_at_head(
        repo_dir.path(),
        "nested-repo/",
        &GitFileStatus::Untracked,
    )
    .await;
    assert_eq!(dir_content, None);

    // Regular untracked files keep their empty baseline.
    let file_content = LocalDiffStateModel::get_file_content_at_head(
        repo_dir.path(),
        "new-file.txt",
        &GitFileStatus::Untracked,
    )
    .await;
    assert_eq!(file_content, Some(String::new()));
}

#[tokio::test]
async fn renamed_file_content_at_head_reads_old_path() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo_path = repo_dir.path();

    // Set up a real git repo with one committed file, then rename it in the working tree
    // (without committing the rename) so HEAD only knows about the old path.
    run_git_command(repo_path, &["init", "-b", "main"])
        .await
        .expect("git init");
    run_git_command(repo_path, &["config", "user.email", "test@test.com"])
        .await
        .expect("git config email");
    run_git_command(repo_path, &["config", "user.name", "Test"])
        .await
        .expect("git config name");
    std::fs::write(repo_path.join("old.txt"), "hello world\n").expect("write old.txt");
    run_git_command(repo_path, &["add", "old.txt"])
        .await
        .expect("git add");
    run_git_command(repo_path, &["commit", "-m", "initial"])
        .await
        .expect("git commit");

    // Rename in the working tree only — `old.txt` no longer exists at this path, so `git
    // show HEAD:new.txt` would fail (the bug in APP-5111).
    std::fs::rename(repo_path.join("old.txt"), repo_path.join("new.txt"))
        .expect("rename old.txt to new.txt");

    let content = LocalDiffStateModel::get_file_content_at_head(
        repo_path,
        "new.txt",
        &GitFileStatus::Renamed {
            old_path: "old.txt".to_string(),
        },
    )
    .await;

    // The baseline content at HEAD must come from the old path, not the new one, so the code
    // review pane can render a diff instead of "Unable to load file content".
    assert_eq!(content, Some("hello world\n".to_string()));
}

#[tokio::test]
async fn staged_rename_and_modify_produces_non_empty_diff() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo_path = repo_dir.path();

    run_git_command(repo_path, &["init", "-b", "main"])
        .await
        .expect("git init");
    run_git_command(repo_path, &["config", "user.email", "test@test.com"])
        .await
        .expect("git config email");
    run_git_command(repo_path, &["config", "user.name", "Test"])
        .await
        .expect("git config name");
    std::fs::write(
        repo_path.join("old.txt"),
        "line one\nline two\nline three\n",
    )
    .expect("write old.txt");
    run_git_command(repo_path, &["add", "old.txt"])
        .await
        .expect("git add");
    run_git_command(repo_path, &["commit", "-m", "initial"])
        .await
        .expect("git commit");

    // Stage both the rename and a content edit, so nothing is left unstaged (git status
    // reports this as a plain "R " entry with no unstaged component).
    run_git_command(repo_path, &["mv", "old.txt", "new.txt"])
        .await
        .expect("git mv");
    std::fs::write(
        repo_path.join("new.txt"),
        "line one\nline two changed\nline three\n",
    )
    .expect("write new.txt");
    run_git_command(repo_path, &["add", "new.txt"])
        .await
        .expect("git add new.txt");

    let diff = LocalDiffStateModel::get_file_diff(
        repo_path,
        "new.txt",
        &GitFileStatus::Renamed {
            old_path: "old.txt".to_string(),
        },
        false,
        None,
    )
    .await
    .expect("get_file_diff should succeed for a fully staged rename+modify");

    // A fully staged rename with a staged content edit must still render an inline diff
    // instead of falling through to "File renamed without changes": comparing only the
    // index against the working tree (as before the fix) produced an empty diff here,
    // since both changes were already staged.
    assert!(
        !diff.is_empty(),
        "expected a non-empty diff for a fully staged rename+modify"
    );
}

#[tokio::test]
async fn num_lines_in_file_if_non_binary_counts_lines_in_text_file() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let file_path = dir.path().join("file.txt");
    std::fs::write(&file_path, "one\ntwo\nthree\n").expect("write file");

    let num_lines = LocalDiffStateModel::num_lines_in_file_if_non_binary(&file_path)
        .await
        .expect("counting a regular file should succeed");
    assert_eq!(num_lines, Some(3));
}

#[tokio::test]
async fn num_lines_in_file_if_non_binary_errors_for_directory() {
    let dir = tempfile::tempdir().expect("create temp dir");

    // Directories aren't countable. The metadata callers degrade this error
    // to a 0-line contribution per entry instead of failing the whole
    // metadata computation.
    let result = LocalDiffStateModel::num_lines_in_file_if_non_binary(dir.path()).await;
    assert!(result.is_err());
}

async fn git(repo: &Path, args: &[&str]) -> String {
    run_git_command(repo, args)
        .await
        .unwrap_or_else(|err| panic!("git {args:?} failed: {err}"))
        .trim()
        .to_string()
}

async fn commit_file(repo: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(repo.join(name), content).expect("write file");
    git(repo, &["add", "-A"]).await;
    git(repo, &["commit", "-m", message]).await;
}

/// A repository whose `feature` branch is checked out and has diverged from `main`: both have
/// one commit the other lacks. Returns the commit they share.
async fn init_diverged_repo(repo: &Path) -> String {
    git(repo, &["init", "-b", "main"]).await;
    git(repo, &["config", "user.email", "test@test.com"]).await;
    git(repo, &["config", "user.name", "Test"]).await;
    git(repo, &["config", "commit.gpgsign", "false"]).await;
    commit_file(repo, "shared.txt", "one\n", "shared").await;
    let fork_point = git(repo, &["rev-parse", "HEAD"]).await;

    git(repo, &["checkout", "-b", "feature"]).await;
    commit_file(repo, "feature.txt", "feature\n", "on feature").await;
    git(repo, &["checkout", "main"]).await;
    commit_file(repo, "shared.txt", "one\ntwo\n", "on main").await;
    git(repo, &["checkout", "feature"]).await;
    fork_point
}

fn revision(rev: &str, comparison: RevisionComparison) -> DiffMode {
    DiffMode::Revision {
        rev: rev.to_string(),
        comparison,
    }
}

async fn changed_paths(repo: &Path, mode: DiffMode) -> Vec<String> {
    let mut paths: Vec<String> =
        LocalDiffStateModel::load_diff_data_for_mode(mode, repo.to_path_buf())
            .await
            .expect("diff loads")
            .files
            .into_iter()
            .map(|file| file.file_path)
            .collect();
    paths.sort_unstable();
    paths
}

#[tokio::test]
async fn resolve_diff_base_follows_the_comparison_of_a_revision() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    let fork_point = init_diverged_repo(repo).await;
    let main = git(repo, &["rev-parse", "main"]).await;

    let direct =
        LocalDiffStateModel::resolve_diff_base(repo, &revision("main", RevisionComparison::Direct))
            .await
            .expect("direct base resolves");
    assert_eq!(direct, main);

    let merge_base = LocalDiffStateModel::resolve_diff_base(
        repo,
        &revision("main", RevisionComparison::MergeBase),
    )
    .await
    .expect("merge base resolves");
    assert_eq!(merge_base, fork_point);

    // A branch compared by name keeps comparing from the merge base.
    let branch =
        LocalDiffStateModel::resolve_diff_base(repo, &DiffMode::OtherBranch("main".to_string()))
            .await
            .expect("branch base resolves");
    assert_eq!(branch, fork_point);

    let expression = LocalDiffStateModel::resolve_diff_base(
        repo,
        &revision("HEAD~1", RevisionComparison::Direct),
    )
    .await
    .expect("expression resolves");
    assert_eq!(expression, fork_point);
}

#[tokio::test]
async fn resolve_diff_base_fails_for_an_unknown_revision_and_for_head() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_diverged_repo(repo).await;

    for comparison in [RevisionComparison::Direct, RevisionComparison::MergeBase] {
        let error =
            LocalDiffStateModel::resolve_diff_base(repo, &revision("no-such-rev", comparison))
                .await
                .expect_err("unknown revision does not resolve");
        assert!(error.to_string().contains("no-such-rev"), "{error}");
    }
    assert!(
        LocalDiffStateModel::resolve_diff_base(repo, &DiffMode::Head)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn revision_mode_diffs_directly_or_from_the_merge_base() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_diverged_repo(repo).await;
    std::fs::write(repo.join("untracked.txt"), "new\n").expect("write untracked file");

    // From the merge base only this branch's own work shows.
    assert_eq!(
        changed_paths(repo, revision("main", RevisionComparison::MergeBase)).await,
        ["feature.txt", "untracked.txt"]
    );
    // Directly, what main gained since the branches diverged shows as well.
    assert_eq!(
        changed_paths(repo, revision("main", RevisionComparison::Direct)).await,
        ["feature.txt", "shared.txt", "untracked.txt"]
    );
    // Comparing with the branch's own previous commit shows what that commit added.
    assert_eq!(
        changed_paths(repo, revision("HEAD~1", RevisionComparison::Direct)).await,
        ["feature.txt", "untracked.txt"]
    );
}

#[tokio::test]
async fn revision_mode_reports_an_unknown_revision_as_an_error() {
    let repo_dir = tempfile::tempdir().expect("create temp repo dir");
    let repo = repo_dir.path();
    init_diverged_repo(repo).await;

    let diffs = LocalDiffStateModel::load_diffs_for_repo(
        repo.to_path_buf(),
        revision("no-such-rev", RevisionComparison::Direct),
        false,
    )
    .await;
    match diffs.changes {
        Ok(_) => panic!("an unknown revision should fail to load"),
        Err(error) => assert!(error.contains("no-such-rev"), "{error}"),
    }
}
