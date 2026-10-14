//! Which git actions a repository row of the source control overview offers, and how they are
//! split between buttons on the row and its overflow menu.

use ::hako_remote::protocol::{GitMerge, GitMergeOutcome, GitMergeState};

use super::git_actions::{SyncOutcome, SyncRefusal};
use super::nested_repos::NestedRepoStatus;
use crate::ui_components::icons::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RepoAction {
    Commit,
    Push,
    /// First push of a branch that has no upstream yet.
    Publish,
    Sync,
    Discard,
    PullRequest,
    Fetch,
    /// Commits the merge in progress, which concludes it.
    CommitMerge,
    AbortMerge,
}

impl RepoAction {
    pub fn label(self) -> &'static str {
        match self {
            RepoAction::Commit => "Commit",
            RepoAction::Push => "Push",
            RepoAction::Publish => "Publish",
            RepoAction::Sync => "Sync",
            RepoAction::Discard => "Discard",
            RepoAction::PullRequest => "Pull request",
            RepoAction::Fetch => "Fetch",
            RepoAction::CommitMerge => "Commit merge",
            RepoAction::AbortMerge => "Abort merge",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            RepoAction::Commit => "Commit changes",
            RepoAction::Push => "Push commits to the remote",
            RepoAction::Publish => "Publish this branch to origin",
            RepoAction::Sync => "Fetch, then fast-forward or push",
            RepoAction::Discard => "Discard all uncommitted changes",
            RepoAction::PullRequest => "View or create the pull request",
            RepoAction::Fetch => "Fetch from the remote",
            RepoAction::CommitMerge => "Commit the merge once its conflicts are resolved",
            RepoAction::AbortMerge => "Abandon the merge and go back to before it",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            RepoAction::Commit => Icon::GitCommit,
            RepoAction::Push => Icon::ArrowUp,
            RepoAction::Publish => Icon::UploadCloud,
            RepoAction::Sync => Icon::SwitchVertical02,
            RepoAction::Discard => Icon::ReverseLeft,
            RepoAction::PullRequest => Icon::Github,
            RepoAction::Fetch => Icon::Download,
            RepoAction::CommitMerge => Icon::GitCommit,
            RepoAction::AbortMerge => Icon::ReverseLeft,
        }
    }

    /// Whether the action gets its own button on the row when there is room for one, rather
    /// than only an entry in the overflow menu.
    fn has_row_button(self) -> bool {
        match self {
            RepoAction::Commit
            | RepoAction::Push
            | RepoAction::Publish
            | RepoAction::Sync
            | RepoAction::Discard
            | RepoAction::CommitMerge
            | RepoAction::AbortMerge => true,
            RepoAction::PullRequest | RepoAction::Fetch => false,
        }
    }
}

/// The actions that apply to a repository in `status`, most relevant first. While a merge is in
/// progress, finishing or abandoning it is all there is to do.
pub fn applicable_actions(status: &NestedRepoStatus) -> Vec<RepoAction> {
    if status.merge.is_some() {
        return vec![RepoAction::CommitMerge, RepoAction::AbortMerge];
    }
    let has_changes = !status.files.is_empty();
    let is_on_branch = !status.is_detached && !status.branch.is_empty();
    let has_commits = status.head_oid.is_some();

    let mut actions = Vec::new();
    if has_changes {
        actions.push(RepoAction::Commit);
    }
    if is_on_branch && has_commits {
        match &status.upstream {
            Some(_) => {
                if status.ahead > 0 {
                    actions.push(RepoAction::Push);
                }
            }
            None => actions.push(RepoAction::Publish),
        }
        actions.push(RepoAction::Sync);
    }
    if has_changes {
        actions.push(RepoAction::Discard);
    }
    if is_on_branch && has_commits && status.upstream.is_some() {
        actions.push(RepoAction::PullRequest);
    }
    if has_commits {
        actions.push(RepoAction::Fetch);
    }
    actions
}

/// How much room a repository row has for action buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowWidth {
    Wide,
    Medium,
    Narrow,
}

/// Rows at least this wide show their action buttons with text labels.
pub const WIDE_ROW_MIN_WIDTH: f32 = 560.;

/// Rows narrower than this keep a single action button and move the rest into the overflow menu.
pub const MEDIUM_ROW_MIN_WIDTH: f32 = 340.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowActions {
    /// Actions shown as buttons on the row, in order.
    pub buttons: Vec<RepoAction>,
    /// Actions reachable through the row's overflow menu, in order.
    pub overflow: Vec<RepoAction>,
    /// Whether the buttons carry their text label next to the icon.
    pub shows_labels: bool,
}

/// Splits a repository's actions between row buttons and the overflow menu for a row of the
/// given width.
pub fn layout_actions(actions: &[RepoAction], width: RowWidth) -> RowActions {
    let max_buttons = match width {
        RowWidth::Wide | RowWidth::Medium => usize::MAX,
        RowWidth::Narrow => 1,
    };
    let mut buttons = Vec::new();
    let mut overflow = Vec::new();
    for action in actions {
        if action.has_row_button() && buttons.len() < max_buttons {
            buttons.push(*action);
        } else {
            overflow.push(*action);
        }
    }
    RowActions {
        buttons,
        overflow,
        shows_labels: width == RowWidth::Wide,
    }
}

/// A one-line report of what an action did, shown on the repository's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowMessage {
    pub text: String,
    pub is_error: bool,
}

impl RowMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

fn commit_count(count: usize) -> String {
    if count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{count} commits")
    }
}

pub fn sync_message(outcome: SyncOutcome, upstream: Option<&str>) -> RowMessage {
    let upstream = upstream.unwrap_or("its upstream");
    match outcome {
        SyncOutcome::Completed {
            pulled: 0,
            pushed: 0,
        } => RowMessage::info(format!("Already up to date with {upstream}")),
        SyncOutcome::Completed { pulled, pushed: 0 } => {
            RowMessage::info(format!("Pulled {} from {upstream}", commit_count(pulled)))
        }
        SyncOutcome::Completed { pulled: 0, pushed } => {
            RowMessage::info(format!("Pushed {} to {upstream}", commit_count(pushed)))
        }
        SyncOutcome::Completed { pulled, pushed } => RowMessage::info(format!(
            "Pulled {} and pushed {}",
            commit_count(pulled),
            commit_count(pushed)
        )),
        SyncOutcome::Refused(SyncRefusal::Diverged { ahead, behind }) => {
            RowMessage::error(format!(
                "Diverged from {upstream} ({ahead} ahead, {behind} behind). Rebase or merge in a \
             terminal."
            ))
        }
        SyncOutcome::Refused(SyncRefusal::NoUpstream) => {
            RowMessage::error("No upstream branch. Publish the branch first.")
        }
        SyncOutcome::Refused(SyncRefusal::OperationInProgress) => operation_in_progress_message(),
    }
}

/// What the row of a repository in the middle of a merge says, such as
/// `Merging feature: 2 conflicts`.
pub fn merge_banner(merge: &GitMergeState) -> String {
    let what = match &merge.branch {
        Some(branch) => format!("Merging {branch}"),
        None => "Merging".to_owned(),
    };
    match merge.conflicts.len() {
        0 => format!("{what}: ready to commit"),
        1 => format!("{what}: 1 conflict"),
        count => format!("{what}: {count} conflicts"),
    }
}

/// The report of a merge that ran to its end, or stopped at conflicts.
pub fn merge_message(merge: &GitMerge, branch: &str) -> RowMessage {
    match merge.outcome {
        GitMergeOutcome::UpToDate => RowMessage::info(format!("Already up to date with {branch}")),
        GitMergeOutcome::FastForward => RowMessage::info(format!("Fast-forwarded to {branch}")),
        GitMergeOutcome::Merged => RowMessage::info(format!("Merged {branch}")),
        // The row goes on to show the merge in progress, so this is no error of its own.
        GitMergeOutcome::Conflicts => RowMessage::info(format!(
            "Merging {branch} stopped at {}",
            conflict_count(merge.conflicts.len())
        )),
    }
}

fn conflict_count(count: usize) -> String {
    if count == 1 {
        "1 conflict".to_owned()
    } else {
        format!("{count} conflicts")
    }
}

/// What to show when git refused a merge or its abort: its own explanation, which says what
/// to do, without its hints.
pub fn merge_refusal(output: &str) -> RowMessage {
    if output.contains("would be overwritten by merge") {
        return RowMessage::error(
            "The merge would overwrite uncommitted changes. Commit or discard them first.",
        );
    }
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .collect();
    let explanation = lines
        .iter()
        .find_map(|line| {
            line.strip_prefix("error: ")
                .or_else(|| line.strip_prefix("fatal: "))
        })
        .or_else(|| lines.first().copied())
        .unwrap_or("git did not say why");
    RowMessage::error(explanation)
}

pub fn operation_in_progress_message() -> RowMessage {
    RowMessage::error("Another git operation is in progress. Finish or abort it first.")
}

#[cfg(test)]
#[path = "nested_repo_actions_tests.rs"]
mod tests;
