use std::fs;

use super::*;
use crate::git_test_support::environment;

fn git(limit: Duration) -> Git {
    Git::new(None, &environment(), limit)
}

#[test]
fn keeps_the_end_of_output_that_is_too_long() {
    // Three bytes each, so that the limit falls inside a character.
    let long = "\u{20ac}".repeat(OUTPUT_LIMIT);

    assert_eq!(tail("  short \n"), "short");
    assert_eq!(tail(&long).len(), OUTPUT_LIMIT - 1);
    assert!(tail(&format!("{long} the end\n")).ends_with("\u{20ac} the end"));
}

#[tokio::test]
async fn gathers_what_git_prints_and_how_it_ended() {
    let directory = tempfile::tempdir().unwrap();
    let git = git(COMMAND_LIMIT);

    let version = git.read(directory.path(), &["--version"]).await;
    let refused = git
        .read(directory.path(), &["rev-parse", "--verify", "HEAD"])
        .await;

    assert!(version.succeeded());
    assert!(version.stdout_text().starts_with("git version"));
    assert_eq!(refused.exit, Exit::Status(128));
    let error = failure(&[&version, &refused]);
    assert_eq!(error.code, ErrorCode::GitFailed);
    assert!(error.message.starts_with("git version"));
    assert!(error.message.contains("not a git repository"));
}

#[tokio::test]
async fn passes_standard_input_on() {
    let directory = tempfile::tempdir().unwrap();

    let hashed = git(COMMAND_LIMIT)
        .change(
            directory.path(),
            &["hash-object", "--stdin"],
            Some(b"hello\n"),
        )
        .await;

    assert_eq!(
        hashed.stdout_text().trim(),
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
}

#[tokio::test]
async fn stops_reading_once_git_has_printed_more_than_asked_for() {
    let directory = tempfile::tempdir().unwrap();

    let version = git(COMMAND_LIMIT)
        .read_at_most(directory.path(), &["--version"], 4)
        .await;

    assert_eq!(version.exit, Exit::Overflowed);
}

#[tokio::test]
async fn stops_git_and_what_it_started_at_the_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let alias = format!(
        "alias.wait=!echo $$ > '{}'; exec sleep 30",
        pid_file.display()
    );
    let limit = Duration::from_secs(1);
    let started = Instant::now();

    let run = git(limit)
        .change(directory.path(), &["-c", &alias, "wait"], None)
        .await;

    assert_eq!(run.exit, Exit::Stopped { after: limit });
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(
        failure(&[&run])
            .message
            .ends_with("Stopped after 1 seconds.")
    );
    let sleeper: libc::pid_t = fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // A process that has ended is still there until it has been waited for.
    let gone_by = Instant::now() + Duration::from_secs(5);
    // SAFETY: signal 0 is not sent; it only asks whether the process exists.
    while unsafe { libc::kill(sleeper, 0) } == 0 {
        assert!(Instant::now() < gone_by, "sleep outlived git");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
