use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::protocol::{NOTIFY_BODY_MAX_CHARS, NOTIFY_TITLE_MAX_CHARS, Notify, NotifyKind};

fn waiting(wait: AgentWait, message: &str) -> AgentReport {
    AgentReport::Waiting {
        wait,
        message: Some(message.to_owned()),
    }
}

fn finished(response: &str) -> AgentReport {
    AgentReport::Finished {
        response: Some(response.to_owned()),
    }
}

fn notify(session_id: &str, title: &str) -> Notify {
    Notify {
        kind: NotifyKind::AgentWaiting,
        title: title.to_owned(),
        body: String::new(),
        session_id: session_id.to_owned(),
        tab_id: None,
        preview_id: None,
    }
}

#[test]
fn tells_when_an_agent_starts_waiting_and_again_only_for_a_new_message() {
    let mut notices = AgentNotices::default();

    assert_eq!(notices.report(1, "Claude Code", AgentReport::Working), None);
    let first = notices
        .report(
            1,
            "Claude Code",
            waiting(AgentWait::Permission, "Wants to run Bash: ls"),
        )
        .unwrap();
    assert_eq!(first.kind, NotifyKind::AgentWaiting);
    assert_eq!(first.title, "Claude Code needs your permission");
    assert_eq!(first.detail.as_deref(), Some("Wants to run Bash: ls"));

    assert_eq!(
        notices.report(
            1,
            "Claude Code",
            waiting(AgentWait::Permission, "Wants to run Bash: ls")
        ),
        None
    );
    assert!(
        notices
            .report(
                1,
                "Claude Code",
                waiting(AgentWait::Permission, "Wants to run Bash: pwd")
            )
            .is_some()
    );
}

#[test]
fn names_what_the_agent_waits_for() {
    let title = |wait| {
        AgentNotices::default()
            .report(1, "Claude Code", waiting(wait, "x"))
            .unwrap()
            .title
    };

    assert_eq!(title(AgentWait::Question), "Claude Code has a question");
    assert_eq!(title(AgentWait::Input), "Claude Code is waiting for you");
}

#[test]
fn tells_once_when_an_agent_finishes_or_fails() {
    let mut notices = AgentNotices::default();
    notices.report(1, "Claude Code", AgentReport::Working);

    let done = notices
        .report(1, "Claude Code", finished("Fixed the login copy."))
        .unwrap();
    assert_eq!(done.kind, NotifyKind::AgentDone);
    assert_eq!(done.title, "Claude Code has finished");
    assert_eq!(done.detail.as_deref(), Some("Fixed the login copy."));
    assert_eq!(notices.report(1, "Claude Code", finished("Again")), None);

    let failed = notices
        .report(
            1,
            "Claude Code",
            AgentReport::Failed {
                error: Some("rate_limit".to_owned()),
                message: Some("Usage limit reached".to_owned()),
            },
        )
        .unwrap();
    assert_eq!(failed.kind, NotifyKind::AgentDone);
    assert_eq!(failed.title, "Claude Code failed");
    assert_eq!(
        failed.detail.as_deref(),
        Some("rate_limit: Usage limit reached")
    );
}

#[test]
fn says_a_failure_failed_even_without_a_message() {
    let notice = AgentNotices::default()
        .report(
            1,
            "Claude Code",
            AgentReport::Failed {
                error: None,
                message: None,
            },
        )
        .unwrap();

    assert_eq!(notice.detail.as_deref(), Some("Stopped with an error"));
}

#[test]
fn does_not_tell_about_working_or_cancelled() {
    let mut notices = AgentNotices::default();

    assert_eq!(notices.report(1, "Claude Code", AgentReport::Working), None);
    assert_eq!(
        notices.report(1, "Claude Code", AgentReport::Cancelled),
        None
    );
    assert!(notices.report(1, "Claude Code", finished("Done")).is_some());
}

#[test]
fn keeps_agents_of_different_terminals_apart() {
    let mut notices = AgentNotices::default();

    assert!(notices.report(1, "Claude Code", finished("a")).is_some());
    assert!(notices.report(2, "Claude Code", finished("b")).is_some());
    notices.forget(&1);
    assert!(notices.report(1, "Claude Code", finished("c")).is_some());
}

#[test]
fn describes_a_long_command() {
    let notice = command_notice("cargo build\n--release", false, 0, Duration::from_secs(134));

    assert_eq!(notice.kind, NotifyKind::CommandDone);
    assert_eq!(notice.title, "cargo build finished");
    assert_eq!(
        notice.detail.as_deref(),
        Some("Took 2 min 14 s, exit status 0")
    );
    assert_eq!(
        command_notice("  ", true, 1, Duration::from_secs(31)).title,
        "Command failed"
    );
}

#[test]
fn cuts_a_long_command_and_keeps_the_outcome() {
    let title = command_notice(&"x".repeat(500), true, 2, Duration::from_secs(40)).title;

    assert!(title.ends_with("… failed"));
    assert_eq!(title.chars().count(), NOTIFY_TITLE_MAX_CHARS);
}

#[test]
fn writes_durations_the_way_people_read_them() {
    assert_eq!(duration_text(Duration::from_secs(31)), "31 s");
    assert_eq!(duration_text(Duration::from_secs(120)), "2 min");
    assert_eq!(duration_text(Duration::from_secs(3780)), "1 h 3 min");
    assert_eq!(duration_text(Duration::from_secs(7200)), "2 h");
}

#[test]
fn names_the_tab_and_the_folder() {
    assert_eq!(
        place(Some("fix login"), Some("hako-remote")).as_deref(),
        Some("fix login in hako-remote")
    );
    assert_eq!(place(Some("zsh"), None).as_deref(), Some("zsh"));
    assert_eq!(place(Some(" "), Some("warp")).as_deref(), Some("warp"));
    assert_eq!(place(None, None), None);
}

#[test]
fn makes_plain_single_lines_of_terminal_text() {
    assert_eq!(
        one_line("\u{1b}[31mred\u{1b}[0m  and\r\n\tthen", 100),
        "red and then"
    );
    assert_eq!(
        one_line("a\u{1b}]0;title\u{7}b\u{1b}]8;;x\u{1b}\\c", 100),
        "abc"
    );
    assert_eq!(
        one_line("left\u{202e}right\u{2028}end", 100),
        "left right end"
    );
}

#[test]
fn cuts_text_on_character_boundaries() {
    let cut = one_line(&"é".repeat(150), NOTIFY_TITLE_MAX_CHARS);

    assert_eq!(cut.chars().count(), NOTIFY_TITLE_MAX_CHARS);
    assert!(cut.ends_with('…'));
    assert_eq!(one_line("ab cd", 4), "ab…");
}

#[test]
fn builds_the_notify_with_the_detail_and_the_place_on_their_own_lines() {
    let notice = Notice {
        kind: NotifyKind::AgentWaiting,
        title: "Claude Code needs your permission".to_owned(),
        detail: Some("Wants to run\nBash: pnpm test".to_owned()),
    };

    let notify = notice.into_notify(
        "s1".to_owned(),
        Some("7".to_owned()),
        Some("fix login in hako-remote"),
    );

    assert_eq!(
        serde_json::to_value(&notify).unwrap(),
        json!({
            "kind": "agent_waiting",
            "title": "Claude Code needs your permission",
            "body": "Wants to run Bash: pnpm test\nfix login in hako-remote",
            "session_id": "s1",
            "tab_id": "7",
        })
    );
}

#[test]
fn keeps_the_place_whole_when_the_detail_is_long() {
    let notice = Notice {
        kind: NotifyKind::AgentDone,
        title: "Claude Code has finished".to_owned(),
        detail: Some("x".repeat(400)),
    };

    let notify = notice.into_notify("s1".to_owned(), None, Some("tab in folder"));

    assert_eq!(notify.body.chars().count(), NOTIFY_BODY_MAX_CHARS);
    assert!(notify.body.ends_with("…\ntab in folder"));
    assert_eq!(
        serde_json::to_value(&notify).unwrap()["tab_id"],
        json!(null)
    );
}

#[test]
fn keeps_the_newest_of_each_terminal_for_a_while() {
    let start = Instant::now();
    let mut pending = PendingNotifications::default();

    pending.keep(notify("a", "first"), start);
    pending.keep(notify("b", "other"), start);
    pending.keep(notify("a", "second"), start + Duration::from_secs(10));

    let sent = pending.take(start + PENDING_MAX_AGE + Duration::from_secs(5));
    assert_eq!(sent, vec![notify("a", "second")]);
    assert!(pending.take(start).is_empty());
}

#[test]
fn keeps_only_a_few_terminals() {
    let now = Instant::now();
    let mut pending = PendingNotifications::default();

    for session in 0..PENDING_MAX_SESSIONS + 2 {
        pending.keep(notify(&session.to_string(), "t"), now);
    }

    let sent = pending.take(now);
    assert_eq!(sent.len(), PENDING_MAX_SESSIONS);
    assert_eq!(sent[0].session_id, "2");
}
