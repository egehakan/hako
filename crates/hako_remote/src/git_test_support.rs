//! Throwaway repositories for the source control tests, made by a git that does not read the
//! configuration of whoever runs the tests.

use std::fs;

use command::blocking::Command;

const ENVIRONMENT: [(&str, &str); 3] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// What the git under test is given on top of its own environment.
pub fn environment() -> Vec<(String, String)> {
    ENVIRONMENT
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// Runs git in `directory` and returns what it printed, trimmed. Panics when git fails.
pub fn git(directory: &str, args: &[&str]) -> String {
    run(directory, args, &[])
}

/// Like [`git`], for a command that makes a commit, which is then written and committed at
/// `time`, in seconds since the epoch.
pub fn git_at(directory: &str, args: &[&str], time: i64) -> String {
    let date = format!("@{time} +0000");
    run(
        directory,
        args,
        &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
    )
}

fn run(directory: &str, args: &[&str], dates: &[(&str, &str)]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .envs(ENVIRONMENT)
        .envs(dates.iter().copied())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed in {directory}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Lets commits be made in the repository at `path`, which is the only place the name is set.
pub fn identify(path: &str) {
    git(path, &["config", "user.name", "Test"]);
    git(path, &["config", "user.email", "test@example.com"]);
}

/// Makes a repository at `path` with one commit on the branch `main`.
pub fn repository(path: &str) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    identify(path);
    write(path, "README.md", "first\n");
    commit_all(path, "First");
}

pub fn write(repo: &str, file: &str, content: impl AsRef<[u8]>) {
    fs::write(format!("{repo}/{file}"), content).unwrap();
}

pub fn commit_all(repo: &str, message: &str) {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", message]);
}

pub fn commit_all_at(repo: &str, message: &str, time: i64) {
    git(repo, &["add", "-A"]);
    git_at(repo, &["commit", "-q", "-m", message], time);
}
