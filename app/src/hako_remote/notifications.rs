//! The `notify` events: a coding agent that waits for the owner or has finished, and a long
//! command that has ended. Finding the terminal takes no terminal model lock.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ::hako_remote::notify::{
    AgentNotices, AgentReport, AgentWait, Notice, PendingNotifications, command_notice, place,
};
use ::hako_remote::protocol::Notify;
use warpui::{AppContext, EntityId, SingletonEntity};

use super::snapshot::{session_id, tab_id, workspace_windows};
use crate::terminal::cli_agent_sessions::{
    BlockedSource, CLIAgentSessionContext, CLIAgentSessionStatus, CLIAgentSessionsModel,
    CLIAgentSessionsModelEvent,
};
use crate::workspace::tab_group::TabGroup;

/// The tool Claude Code asks the owner a question with. The question reaches the app as a
/// request for permission to use it.
const QUESTION_TOOL: &str = "AskUserQuestion";
const QUESTION_FIELD: &str = "\"question\":\"";

/// A command that has ended.
pub(crate) struct FinishedCommand {
    pub(crate) command: String,
    pub(crate) exit_status: i32,
    pub(crate) failed: bool,
    pub(crate) duration: Duration,
    pub(crate) finished_at: Instant,
}

#[derive(Default)]
pub(super) struct Notifications {
    agents: AgentNotices<EntityId>,
    /// When the agent in each terminal last reported its status.
    agent_reported_at: HashMap<EntityId, Instant>,
    /// When the latest command of each terminal started. A command written straight to the
    /// terminal, as a remote client's input is, rather than run from the input editor leaves its
    /// block without a start time.
    command_started_at: HashMap<EntityId, Instant>,
    pub(super) pending: PendingNotifications,
}

impl Notifications {
    /// The notification a change of a coding agent's session calls for, if any.
    pub(super) fn agent_event(
        &mut self,
        event: &CLIAgentSessionsModelEvent,
        ctx: &AppContext,
    ) -> Option<Notify> {
        match event {
            CLIAgentSessionsModelEvent::StatusChanged {
                terminal_view_id,
                agent,
                status,
                session_context,
                is_prompt_submit: _,
            } => {
                // Clients are shown an agent's status only once it reports one that can be relied
                // on, so its notifications start there too.
                let is_reliable = CLIAgentSessionsModel::as_ref(ctx)
                    .session(*terminal_view_id)
                    .is_some_and(|session| session.supports_rich_status());
                if !is_reliable {
                    return None;
                }
                self.agent_reported_at
                    .insert(*terminal_view_id, Instant::now());
                let notice = self.agents.report(
                    *terminal_view_id,
                    agent.display_name(),
                    agent_report(status, session_context),
                )?;
                locate(*terminal_view_id, notice, ctx)
            }
            CLIAgentSessionsModelEvent::Ended {
                terminal_view_id, ..
            } => {
                self.agents.forget(terminal_view_id);
                None
            }
            CLIAgentSessionsModelEvent::Started { .. }
            | CLIAgentSessionsModelEvent::InputSessionChanged { .. }
            | CLIAgentSessionsModelEvent::SessionUpdated { .. } => None,
        }
    }

    pub(super) fn command_started(&mut self, terminal_view_id: EntityId, started_at: Instant) {
        self.command_started_at.insert(terminal_view_id, started_at);
    }

    pub(super) fn command_started_at(&self, terminal_view_id: EntityId) -> Option<Instant> {
        self.command_started_at.get(&terminal_view_id).copied()
    }

    /// The notification for a long command, unless an agent that ran in it has already said
    /// what happened.
    pub(super) fn command_finished(
        &mut self,
        terminal_view_id: EntityId,
        command: FinishedCommand,
        ctx: &AppContext,
    ) -> Option<Notify> {
        let started_at = command.finished_at.checked_sub(command.duration);
        let agent_reported =
            self.agent_reported_at
                .get(&terminal_view_id)
                .is_some_and(|reported_at| {
                    started_at.is_none_or(|started_at| *reported_at >= started_at)
                });
        if agent_reported {
            return None;
        }
        let notice = command_notice(
            &command.command,
            command.failed,
            command.exit_status,
            command.duration,
        );
        locate(terminal_view_id, notice, ctx)
    }
}

fn agent_report(status: &CLIAgentSessionStatus, context: &CLIAgentSessionContext) -> AgentReport {
    match status {
        CLIAgentSessionStatus::InProgress => AgentReport::Working,
        CLIAgentSessionStatus::Blocked { message, source } => match source {
            BlockedSource::PermissionRequest
                if context.tool_name.as_deref() == Some(QUESTION_TOOL) =>
            {
                AgentReport::Waiting {
                    wait: AgentWait::Question,
                    message: message.as_deref().and_then(question),
                }
            }
            BlockedSource::PermissionRequest => AgentReport::Waiting {
                wait: AgentWait::Permission,
                message: message.clone(),
            },
            BlockedSource::QuestionAsked => AgentReport::Waiting {
                wait: AgentWait::Question,
                message: message.clone(),
            },
            BlockedSource::NeedsInput => AgentReport::Waiting {
                wait: AgentWait::Input,
                message: message.clone(),
            },
        },
        CLIAgentSessionStatus::Success => AgentReport::Finished {
            response: context.response.clone(),
        },
        CLIAgentSessionStatus::Failed {
            error_type,
            message,
        } => AgentReport::Failed {
            error: error_type.clone(),
            message: message.clone(),
        },
        CLIAgentSessionStatus::Cancelled => AgentReport::Cancelled,
    }
}

/// The first question in the summary of a request to use [`QUESTION_TOOL`], which holds the
/// start of the tool's input as JSON.
fn question(summary: &str) -> Option<String> {
    let start = summary.find(QUESTION_FIELD)? + QUESTION_FIELD.len();
    let rest = &summary[start..];
    let mut question = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return (!question.is_empty()).then_some(question),
            '\\' => match chars.next() {
                Some('n') => question.push(' '),
                Some(escaped) => question.push(escaped),
                None => break,
            },
            c => question.push(c),
        }
    }
    // The summary ends inside the question.
    let question = question.trim_end_matches('.').trim_end();
    (!question.is_empty()).then(|| format!("{question}…"))
}

/// The notify for the terminal of `terminal_view_id`, which must still be in a tab.
fn locate(terminal_view_id: EntityId, notice: Notice, ctx: &AppContext) -> Option<Notify> {
    for (_, workspace) in workspace_windows(ctx) {
        let workspace = workspace.as_ref(ctx);
        for (index, tab) in workspace.tabs.iter().enumerate() {
            let pane_group = tab.pane_group.as_ref(ctx);
            let Some(pane_id) = pane_group.find_pane_id_for_terminal_view(terminal_view_id, ctx)
            else {
                continue;
            };
            let folder_name = tab
                .group_id
                .and_then(|group_id| workspace.tab_groups.get(&group_id))
                .filter(|group| TabGroup::is_folder(group))
                .map(TabGroup::display_name);
            let place = place(
                Some(&workspace.tab_title(index, ctx)),
                folder_name.as_deref(),
            );
            return Some(notice.into_notify(
                session_id(pane_group, pane_id)?,
                Some(tab_id(tab.pane_group.id())),
                place.as_deref(),
            ));
        }
    }
    None
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
