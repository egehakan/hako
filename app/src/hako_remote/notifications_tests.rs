use ::hako_remote::notify::{AgentReport, AgentWait};

use super::{agent_report, question};
use crate::terminal::cli_agent_sessions::{
    BlockedSource, CLIAgentSessionContext, CLIAgentSessionStatus,
};

fn blocked(source: BlockedSource, message: &str) -> CLIAgentSessionStatus {
    CLIAgentSessionStatus::Blocked {
        message: Some(message.to_owned()),
        source,
    }
}

fn context_with_tool(tool_name: &str) -> CLIAgentSessionContext {
    CLIAgentSessionContext {
        tool_name: Some(tool_name.to_owned()),
        ..Default::default()
    }
}

#[test]
fn maps_each_status_of_a_session() {
    let context = CLIAgentSessionContext {
        response: Some("All tests pass.".to_owned()),
        ..Default::default()
    };

    assert_eq!(
        agent_report(&CLIAgentSessionStatus::InProgress, &context),
        AgentReport::Working
    );
    assert_eq!(
        agent_report(&CLIAgentSessionStatus::Success, &context),
        AgentReport::Finished {
            response: Some("All tests pass.".to_owned())
        }
    );
    assert_eq!(
        agent_report(
            &CLIAgentSessionStatus::Failed {
                error_type: Some("rate_limit".to_owned()),
                message: None,
            },
            &context
        ),
        AgentReport::Failed {
            error: Some("rate_limit".to_owned()),
            message: None,
        }
    );
    assert_eq!(
        agent_report(&CLIAgentSessionStatus::Cancelled, &context),
        AgentReport::Cancelled
    );
    assert_eq!(
        agent_report(
            &blocked(BlockedSource::NeedsInput, "Agent requires user input"),
            &context
        ),
        AgentReport::Waiting {
            wait: AgentWait::Input,
            message: Some("Agent requires user input".to_owned()),
        }
    );
}

#[test]
fn tells_a_question_from_a_permission_request() {
    let summary = r#"Wants to run AskUserQuestion: {"questions":[{"question":"Which \"db\" to use?","header":"DB""#;

    assert_eq!(
        agent_report(
            &blocked(BlockedSource::PermissionRequest, "Wants to run Bash: ls"),
            &context_with_tool("Bash")
        ),
        AgentReport::Waiting {
            wait: AgentWait::Permission,
            message: Some("Wants to run Bash: ls".to_owned()),
        }
    );
    assert_eq!(
        agent_report(
            &blocked(BlockedSource::PermissionRequest, summary),
            &context_with_tool("AskUserQuestion")
        ),
        AgentReport::Waiting {
            wait: AgentWait::Question,
            message: Some("Which \"db\" to use?".to_owned()),
        }
    );
}

#[test]
fn reads_a_question_that_the_summary_cuts_off() {
    assert_eq!(
        question(
            r#"Wants to run AskUserQuestion: {"questions":[{"question":"Should we migrate the..."#
        )
        .as_deref(),
        Some("Should we migrate the…")
    );
    assert_eq!(question("Wants to run AskUserQuestion: {}"), None);
}
