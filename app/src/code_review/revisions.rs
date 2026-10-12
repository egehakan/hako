//! The revisions a working directory can be compared with besides its local branches: remote
//! branches, tags, recent commits, and anything typed that git resolves to a commit.

use std::fmt;
use std::path::Path;

use warp_util::git::run_git_command;

/// How many remote branches, and how many tags, are listed. The most recent come first.
const MAX_LISTED_REFS: usize = 200;

/// How many commits of the current branch are listed.
const MAX_LISTED_COMMITS: usize = 200;

const REMOTE_BRANCH_PREFIX: &str = "refs/remotes/";
const TAG_PREFIX: &str = "refs/tags/";

/// Fields are separated by tabs, with the subject last because it may itself contain tabs.
const REF_LISTING_FORMAT: &str = "--format=%(refname)%09%(objectname:short)%09%(*objectname:short)%09%(creatordate:relative)%09%(contents:subject)";
const COMMIT_LISTING_FORMAT: &str = "--format=%H%x09%h%x09%an%x09%cr%x09%s";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevisionKind {
    RemoteBranch,
    Tag,
    Commit,
}

/// A revision offered as a comparison base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionEntry {
    pub kind: RevisionKind,
    /// What git is asked to resolve: the short name of a ref, or the full id of a commit.
    pub rev: String,
    /// Abbreviated id of the commit the revision points at.
    pub short_sha: String,
    /// First line of the commit's message, or of an annotated tag's.
    pub subject: String,
    /// Empty for refs, whose listing is about the ref and not about who wrote its commit.
    pub author: String,
    /// Age as git words it, such as "3 days ago".
    pub relative_date: String,
}

/// Everything listed in the comparison picker besides local branches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevisionCandidates {
    pub remote_branches: Vec<RevisionEntry>,
    pub tags: Vec<RevisionEntry>,
    pub commits: Vec<RevisionEntry>,
}

/// Parses the output of `git for-each-ref` run with [`REF_LISTING_FORMAT`].
pub(crate) fn parse_ref_listing(output: &str) -> Vec<RevisionEntry> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let refname = fields.next()?;
            let object = fields.next()?;
            let peeled = fields.next()?;
            let relative_date = fields.next()?;
            let subject = fields.next().unwrap_or_default();

            let (kind, name) = if let Some(name) = refname.strip_prefix(REMOTE_BRANCH_PREFIX) {
                // `origin/HEAD` only points at the remote's default branch, which is listed too.
                if name.ends_with("/HEAD") || !name.contains('/') {
                    return None;
                }
                (RevisionKind::RemoteBranch, name)
            } else {
                (RevisionKind::Tag, refname.strip_prefix(TAG_PREFIX)?)
            };

            // An annotated tag is an object of its own; the commit is what it peels to.
            let short_sha = if peeled.is_empty() { object } else { peeled };
            Some(RevisionEntry {
                kind,
                rev: name.to_string(),
                short_sha: short_sha.to_string(),
                subject: subject.to_string(),
                author: String::new(),
                relative_date: relative_date.to_string(),
            })
        })
        .collect()
}

/// Parses the output of `git log` run with [`COMMIT_LISTING_FORMAT`].
pub(crate) fn parse_commit_listing(output: &str) -> Vec<RevisionEntry> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let sha = fields.next().filter(|sha| !sha.is_empty())?;
            let short_sha = fields.next()?;
            let author = fields.next()?;
            let relative_date = fields.next()?;
            let subject = fields.next().unwrap_or_default();
            Some(RevisionEntry {
                kind: RevisionKind::Commit,
                rev: sha.to_string(),
                short_sha: short_sha.to_string(),
                subject: subject.to_string(),
                author: author.to_string(),
                relative_date: relative_date.to_string(),
            })
        })
        .collect()
}

async fn list_refs(repo_path: &Path, pattern: &str) -> Vec<RevisionEntry> {
    let count = format!("--count={MAX_LISTED_REFS}");
    let args = [
        "for-each-ref",
        count.as_str(),
        "--sort=-creatordate",
        REF_LISTING_FORMAT,
        pattern,
    ];
    match run_git_command(repo_path, &args).await {
        Ok(output) => parse_ref_listing(&output),
        Err(err) => {
            log::warn!("Failed to list {pattern}: {err}");
            Vec::new()
        }
    }
}

/// Lists the remote branches, tags and recent commits of the repository at `repo_path`. A kind
/// that cannot be listed, such as the commits of a repository without any, comes back empty.
pub async fn list_revision_candidates(repo_path: &Path) -> RevisionCandidates {
    let remote_branches = list_refs(repo_path, "refs/remotes").await;
    let tags = list_refs(repo_path, "refs/tags").await;

    let count = format!("--max-count={MAX_LISTED_COMMITS}");
    let commits = match run_git_command(
        repo_path,
        &["log", count.as_str(), COMMIT_LISTING_FORMAT, "HEAD"],
    )
    .await
    {
        Ok(output) => parse_commit_listing(&output),
        Err(_) => Vec::new(),
    };

    RevisionCandidates {
        remote_branches,
        tags,
        commits,
    }
}

/// Why a typed revision cannot be compared with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevisionError {
    Empty,
    /// Two revisions joined by `..` or `...`.
    Range,
    /// Text git would read as an option, or that no revision can contain.
    Malformed,
    NotFound(String),
    /// Resolves to something other than a commit, such as a tree or a file.
    NotACommit(String),
}

impl fmt::Display for RevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RevisionError::Empty => write!(f, "Type a branch, tag or commit"),
            RevisionError::Range => {
                write!(f, "Type one revision, not a range")
            }
            RevisionError::Malformed => write!(f, "Not a valid revision"),
            RevisionError::NotFound(rev) => {
                write!(f, "No branch, tag or commit named '{rev}' in this repository")
            }
            RevisionError::NotACommit(rev) => write!(f, "'{rev}' is not a commit"),
        }
    }
}

/// Checks that typed text can be handed to git as a single revision, and trims it.
pub fn parse_revision_input(input: &str) -> Result<&str, RevisionError> {
    let rev = input.trim();
    if rev.is_empty() {
        return Err(RevisionError::Empty);
    }
    if rev.starts_with('-') || rev.chars().any(char::is_control) {
        return Err(RevisionError::Malformed);
    }
    if rev.contains("..") {
        return Err(RevisionError::Range);
    }
    Ok(rev)
}

/// Resolves `rev` to the full id of the commit it names in the repository at `repo_path`.
pub async fn resolve_commit(repo_path: &Path, rev: &str) -> Result<String, RevisionError> {
    let rev = parse_revision_input(rev)?;
    let object = run_git_command(repo_path, &["rev-parse", "--verify", "--quiet", rev])
        .await
        .map_err(|_| RevisionError::NotFound(rev.to_string()))?;
    let object = object.trim();
    if object.is_empty() {
        return Err(RevisionError::NotFound(rev.to_string()));
    }

    // Peeling in a second step keeps a revision such as `:/message` intact.
    let peeled = format!("{object}^{{commit}}");
    let commit = run_git_command(repo_path, &["rev-parse", "--verify", "--quiet", &peeled])
        .await
        .map_err(|_| RevisionError::NotACommit(rev.to_string()))?;
    let commit = commit.trim();
    if commit.is_empty() {
        return Err(RevisionError::NotACommit(rev.to_string()));
    }
    Ok(commit.to_string())
}

#[cfg(test)]
#[path = "revisions_tests.rs"]
mod tests;
