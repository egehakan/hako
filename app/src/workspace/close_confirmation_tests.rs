use super::*;

const NOTHING_AT_STAKE: CloseStakes = CloseStakes {
    running_commands: 0,
    shared_sessions: 0,
    unsaved_changes: false,
};

fn every_subject() -> Vec<CloseSubject> {
    vec![
        CloseSubject::Pane {
            title: "zsh".to_string(),
        },
        CloseSubject::Tab {
            title: "api".to_string(),
            closes_window: false,
        },
        CloseSubject::Tab {
            title: "api".to_string(),
            closes_window: true,
        },
        CloseSubject::Tabs { count: 3 },
        CloseSubject::Group {
            name: "API".to_string(),
            tab_count: 2,
            removes_group: false,
        },
        CloseSubject::Group {
            name: "API".to_string(),
            tab_count: 2,
            removes_group: true,
        },
        CloseSubject::Group {
            name: "API".to_string(),
            tab_count: 0,
            removes_group: true,
        },
        CloseSubject::Window { tab_count: 4 },
    ]
}

fn asked(subject: &CloseSubject, stakes: CloseStakes) -> CloseConfirmationText {
    match close_decision(subject, stakes, true) {
        CloseDecision::Ask(text) => text,
        CloseDecision::WarnOnlyIfSomethingIsLost => panic!("{subject:?} was not confirmed"),
    }
}

#[test]
fn every_close_is_confirmed_even_when_nothing_is_running() {
    for subject in every_subject() {
        let text = asked(&subject, NOTHING_AT_STAKE);
        assert!(text.title.ends_with('?'), "{subject:?}: {}", text.title);
        assert!(!text.body.contains("running"), "{subject:?}: {}", text.body);
    }
}

#[test]
fn nothing_is_confirmed_up_front_when_the_setting_is_off() {
    let busy = CloseStakes {
        running_commands: 2,
        shared_sessions: 1,
        unsaved_changes: true,
    };
    for subject in every_subject() {
        for stakes in [NOTHING_AT_STAKE, busy] {
            assert_eq!(
                close_decision(&subject, stakes, false),
                CloseDecision::WarnOnlyIfSomethingIsLost,
                "{subject:?}"
            );
        }
    }
}

#[test]
fn the_confirmation_names_what_is_closed() {
    let pane = asked(
        &CloseSubject::Pane {
            title: "zsh".to_string(),
        },
        NOTHING_AT_STAKE,
    );
    assert_eq!(pane.title, "Close this pane?");
    assert_eq!(pane.body, "The pane \"zsh\" will be closed.");
    assert_eq!(pane.confirm_label, "Close pane");

    let tab = asked(
        &CloseSubject::Tab {
            title: "api".to_string(),
            closes_window: false,
        },
        NOTHING_AT_STAKE,
    );
    assert_eq!(tab.title, "Close this tab?");
    assert_eq!(tab.body, "The tab \"api\" will be closed.");
    assert_eq!(tab.confirm_label, "Close tab");

    let last_tab = asked(
        &CloseSubject::Tab {
            title: "api".to_string(),
            closes_window: true,
        },
        NOTHING_AT_STAKE,
    );
    assert!(last_tab.body.contains("the window closes with it"));

    let tabs = asked(&CloseSubject::Tabs { count: 3 }, NOTHING_AT_STAKE);
    assert_eq!(tabs.title, "Close 3 tabs?");
    assert_eq!(tabs.body, "3 tabs will be closed.");

    let window = asked(&CloseSubject::Window { tab_count: 1 }, NOTHING_AT_STAKE);
    assert_eq!(window.title, "Close this window?");
    assert_eq!(window.body, "The window and its 1 tab will be closed.");
    assert_eq!(window.confirm_label, "Close window");
}

#[test]
fn closing_the_tabs_of_a_group_and_removing_it_read_differently() {
    let close = asked(
        &CloseSubject::Group {
            name: "API".to_string(),
            tab_count: 2,
            removes_group: false,
        },
        NOTHING_AT_STAKE,
    );
    assert_eq!(close.title, "Close all tabs in \"API\"?");
    assert!(close.body.contains("The group stays in the tab list."));
    assert_eq!(close.confirm_label, "Close tabs");

    let remove = asked(
        &CloseSubject::Group {
            name: "API".to_string(),
            tab_count: 2,
            removes_group: true,
        },
        NOTHING_AT_STAKE,
    );
    assert_eq!(remove.title, "Remove the group \"API\"?");
    assert!(remove.body.contains("its 2 tabs closed"));
    assert!(remove.body.contains("Nothing on disk is deleted."));
    assert_eq!(remove.confirm_label, "Remove group");

    let remove_empty = asked(
        &CloseSubject::Group {
            name: "API".to_string(),
            tab_count: 0,
            removes_group: true,
        },
        NOTHING_AT_STAKE,
    );
    assert!(!remove_empty.body.contains("closed"));
}

#[test]
fn the_confirmation_says_when_a_command_is_still_running() {
    for subject in every_subject() {
        let one = asked(
            &subject,
            CloseStakes {
                running_commands: 1,
                ..NOTHING_AT_STAKE
            },
        );
        assert!(
            one.body.ends_with(" A command is still running."),
            "{subject:?}: {}",
            one.body
        );

        let several = asked(
            &subject,
            CloseStakes {
                running_commands: 3,
                ..NOTHING_AT_STAKE
            },
        );
        assert!(
            several.body.ends_with(" 3 commands are still running."),
            "{subject:?}: {}",
            several.body
        );
    }
}

#[test]
fn the_confirmation_lists_everything_that_would_be_lost() {
    let text = asked(
        &CloseSubject::Window { tab_count: 2 },
        CloseStakes {
            running_commands: 1,
            shared_sessions: 2,
            unsaved_changes: true,
        },
    );
    assert_eq!(
        text.body,
        "The window and its 2 tabs will be closed. A command is still running. 2 shared \
         sessions will end for everyone. Unsaved changes will be lost."
    );
}

#[test]
fn the_dialog_opens_with_cancel_focused() {
    let dialog = CloseConfirmationDialog::new();
    assert_eq!(dialog.focused_button, DialogButton::Cancel);
}
