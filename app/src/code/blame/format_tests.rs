use super::*;

const NOW: i64 = 1_760_000_000;
const SHA: &str = "2f6510a11a70dd87536a816fb8173acd92ba3199";

fn commit(author: &str, mail: &str, seconds_ago: i64, summary: &str) -> BlameCommit {
    BlameCommit {
        sha: SHA.to_string(),
        author: author.to_string(),
        author_mail: mail.to_string(),
        author_time: NOW - seconds_ago,
        summary: summary.to_string(),
        ..Default::default()
    }
}

fn identity(name: Option<&str>, email: Option<&str>) -> GitIdentity {
    GitIdentity {
        name: name.map(str::to_string),
        email: email.map(str::to_string),
    }
}

#[test]
fn relative_time_picks_the_largest_fitting_unit() {
    let cases = [
        (0, "just now"),
        (59, "just now"),
        (60, "1 minute ago"),
        (59 * 60 + 59, "59 minutes ago"),
        (3600, "1 hour ago"),
        (23 * 3600 + 3599, "23 hours ago"),
        (DAY, "yesterday"),
        (2 * DAY - 1, "yesterday"),
        (2 * DAY, "2 days ago"),
        (13 * DAY, "13 days ago"),
        (14 * DAY, "2 weeks ago"),
        (59 * DAY, "8 weeks ago"),
        (60 * DAY, "2 months ago"),
        (364 * DAY, "12 months ago"),
        (365 * DAY, "1 year ago"),
        (3 * 365 * DAY + 100 * DAY, "3 years ago"),
    ];
    for (seconds_ago, expected) in cases {
        assert_eq!(
            relative_time(NOW - seconds_ago, NOW),
            expected,
            "{seconds_ago}s ago"
        );
    }
}

#[test]
fn relative_time_of_a_commit_from_the_future_is_just_now() {
    assert_eq!(relative_time(NOW + 5000, NOW), "just now");
    assert_eq!(relative_time(i64::MAX, i64::MIN), "just now");
}

#[test]
fn absolute_time_uses_the_authors_offset() {
    assert_eq!(
        absolute_time(1749301750, -300).as_deref(),
        Some("7 June 2025 08:09 -05:00")
    );
    assert_eq!(
        absolute_time(1704161045, 60).as_deref(),
        Some("2 January 2024 03:04 +01:00")
    );
    assert_eq!(absolute_time(0, i32::MAX), None);
}

#[test]
fn you_are_recognised_by_address_or_by_name() {
    let ada = commit("Ada Lovelace", "ada@example.com", 0, "");

    assert!(identity(Some("Someone Else"), Some("ADA@example.com")).is_author_of(&ada));
    assert!(identity(Some("Ada Lovelace"), Some("other@example.com")).is_author_of(&ada));
    assert!(identity(Some(" Ada Lovelace "), None).is_author_of(&ada));
    assert!(identity(None, Some("ada@example.com")).is_author_of(&ada));
}

#[test]
fn others_and_unset_identities_are_not_you() {
    let ada = commit("Ada Lovelace", "ada@example.com", 0, "");

    assert!(!identity(Some("Grace Hopper"), Some("grace@example.com")).is_author_of(&ada));
    assert!(!identity(Some("ada lovelace"), None).is_author_of(&ada));
    assert!(!GitIdentity::default().is_author_of(&ada));

    // An identity with nothing set must not match a commit with nothing recorded.
    let anonymous = commit("", "", 0, "");
    assert!(!identity(Some(""), Some("")).is_author_of(&anonymous));
    assert!(!identity(Some("  "), Some(" ")).is_author_of(&anonymous));
}

#[test]
fn annotation_names_author_age_and_summary() {
    let grace = commit(
        "Grace Hopper",
        "grace@example.com",
        3 * DAY,
        "Fix the parser",
    );

    assert_eq!(
        annotation_text(&grace, &GitIdentity::default(), NOW),
        "Grace Hopper, 3 days ago \u{2022} Fix the parser"
    );
    assert_eq!(
        annotation_text(&grace, &identity(None, Some("grace@example.com")), NOW),
        "You, 3 days ago \u{2022} Fix the parser"
    );
}

#[test]
fn annotation_of_an_uncommitted_line_says_so() {
    let mut unsaved = commit(
        "External file (--contents)",
        "external.file",
        0,
        "Version of a",
    );
    unsaved.sha = "0".repeat(40);

    assert_eq!(
        annotation_text(&unsaved, &GitIdentity::default(), NOW),
        "Not committed yet"
    );
}

#[test]
fn annotation_without_a_summary_has_no_separator() {
    let empty = commit("Ada", "ada@example.com", 7200, "  ");

    assert_eq!(
        annotation_text(&empty, &GitIdentity::default(), NOW),
        "Ada, 2 hours ago"
    );
}

#[test]
fn annotation_cuts_a_long_summary() {
    let long = commit("Ada", "ada@example.com", 60, &"word ".repeat(40));
    let text = annotation_text(&long, &GitIdentity::default(), NOW);
    let summary = text
        .split(" \u{2022} ")
        .nth(1)
        .expect("summary follows the separator");

    assert_eq!(summary.chars().count(), 72);
    assert!(summary.ends_with('\u{2026}'));
    assert!(summary.starts_with("word word"));
}

#[test]
fn truncation_counts_characters_and_keeps_short_text() {
    assert_eq!(truncate_chars("short", 10), "short");
    assert_eq!(truncate_chars("exactly ten", 11), "exactly ten");
    assert_eq!(truncate_chars("exactly ten!", 11), "exactly te\u{2026}");
    // Multi-byte characters count once each and are never split.
    assert_eq!(truncate_chars("größe über alles", 6), "größe\u{2026}");
    assert_eq!(truncate_chars("日本語のテキスト", 4), "日本語\u{2026}");
    // The ellipsis does not follow a space.
    assert_eq!(truncate_chars("one two three", 5), "one\u{2026}");
    assert_eq!(truncate_chars("anything", 0), "\u{2026}");
}

#[test]
fn short_sha_is_seven_characters() {
    assert_eq!(short_sha(SHA), "2f6510a");
    assert_eq!(short_sha("abc"), "abc");
}

#[test]
fn github_url_from_https_remotes() {
    let expected = format!("https://github.com/warpdotdev/warp/commit/{SHA}");
    for remote in [
        "https://github.com/warpdotdev/warp.git",
        "https://github.com/warpdotdev/warp",
        "https://github.com/warpdotdev/warp/",
        "https://GitHub.com/warpdotdev/warp.git\n",
        "http://github.com/warpdotdev/warp.git",
        "https://www.github.com/warpdotdev/warp.git",
        "https://github.com:443/warpdotdev/warp.git",
    ] {
        assert_eq!(
            github_commit_url(remote, SHA).as_deref(),
            Some(expected.as_str()),
            "{remote}"
        );
    }
}

#[test]
fn github_url_leaves_credentials_out() {
    assert_eq!(
        github_commit_url("https://user:secret-token@github.com/owner/repo.git", SHA),
        Some(format!("https://github.com/owner/repo/commit/{SHA}"))
    );
}

#[test]
fn github_url_from_ssh_remotes() {
    let expected = format!("https://github.com/owner/my.repo-name_1/commit/{SHA}");
    for remote in [
        "git@github.com:owner/my.repo-name_1.git",
        "git@github.com:owner/my.repo-name_1",
        "ssh://git@github.com/owner/my.repo-name_1.git",
        "ssh://git@github.com:22/owner/my.repo-name_1.git",
        "git://github.com/owner/my.repo-name_1.git",
        "github.com:owner/my.repo-name_1.git",
    ] {
        assert_eq!(
            github_commit_url(remote, SHA).as_deref(),
            Some(expected.as_str()),
            "{remote}"
        );
    }
}

#[test]
fn no_github_url_for_other_hosts() {
    for remote in [
        "https://gitlab.com/owner/repo.git",
        "git@bitbucket.org:owner/repo.git",
        "https://github.example.com/owner/repo.git",
        "https://github.com.evil.example/owner/repo.git",
        "https://notgithub.com/owner/repo.git",
        "git@github.com.evil.example:owner/repo.git",
        "https://evil.example/github.com/owner/repo.git",
        "/srv/git/github.com/owner/repo.git",
        "../remotes/alpha.git",
        "file:///srv/git/repo.git",
        "",
    ] {
        assert_eq!(github_commit_url(remote, SHA), None, "{remote}");
    }
}

#[test]
fn no_github_url_for_malformed_repositories_or_commits() {
    assert_eq!(github_commit_url("https://github.com/owner", SHA), None);
    assert_eq!(
        github_commit_url("https://github.com/owner/repo/extra", SHA),
        None
    );
    assert_eq!(
        github_commit_url("https://github.com/owner/../repo", SHA),
        None
    );
    assert_eq!(
        github_commit_url("https://github.com/ow ner/repo", SHA),
        None
    );
    assert_eq!(
        github_commit_url("https://github.com/owner/repo.git", ""),
        None
    );
    assert_eq!(
        github_commit_url("https://github.com/owner/repo.git", "HEAD~1"),
        None
    );
}
