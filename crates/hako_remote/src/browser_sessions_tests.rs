use std::time::{Duration, Instant};

use super::*;

fn viewport() -> Viewport {
    Viewport {
        width: 393,
        height: 852,
        scale: 3.0,
        mobile: true,
        touch: true,
        user_agent: None,
    }
}

fn tab(id: &str, opener: Option<&str>) -> Tab {
    Tab::new(id.to_owned(), format!("S-{id}"), opener.map(str::to_owned))
}

#[test]
fn sessions_get_distinct_ids() {
    let mut sessions = Sessions::default();
    let now = Instant::now();
    let first = sessions.open(viewport(), now);
    let second = sessions.open(viewport(), now);
    assert_ne!(first, second);
    assert!(sessions.close(first).is_some());
    assert!(sessions.close(first).is_none());
    assert_eq!(sessions.bids(), [second]);
}

#[test]
fn a_pop_up_becomes_active_and_hands_back_to_its_opener() {
    let mut sessions = Sessions::default();
    let bid = sessions.open(viewport(), Instant::now());
    assert!(sessions.add_tab(bid, tab("main", None), false));
    assert_eq!(sessions.get(bid).unwrap().active_tab().unwrap().id, "main");

    // The page opens a sign-in pop-up, then a tab of its own from the main page.
    assert!(sessions.add_tab(bid, tab("other", None), false));
    assert!(sessions.add_tab(bid, tab("popup", Some("main")), true));
    assert_eq!(sessions.owner("popup"), Some(bid));
    assert!(sessions.is_active("popup"));
    let state = sessions.get(bid).unwrap().state(bid);
    assert_eq!(state.tab.as_deref(), Some("popup"));
    assert_eq!(state.tabs.iter().filter(|tab| tab.active).count(), 1);

    let removed = sessions.remove_tab("popup").unwrap();
    assert_eq!(removed.bid, bid);
    assert_eq!(removed.activated.as_deref(), Some("main"));
    assert!(!removed.empty);

    // A background tab closing changes nothing about which is active.
    let removed = sessions.remove_tab("other").unwrap();
    assert_eq!(removed.activated, None);
    assert!(sessions.is_active("main"));

    let removed = sessions.remove_tab("main").unwrap();
    assert!(removed.empty);
    assert_eq!(removed.activated, None);
    assert!(sessions.remove_tab("main").is_none());
}

#[test]
fn without_its_opener_the_newest_tab_takes_over() {
    let mut sessions = Sessions::default();
    let bid = sessions.open(viewport(), Instant::now());
    sessions.add_tab(bid, tab("a", None), true);
    sessions.add_tab(bid, tab("b", None), false);
    sessions.add_tab(bid, tab("c", Some("gone")), true);
    assert_eq!(
        sessions.remove_tab("c").unwrap().activated.as_deref(),
        Some("b")
    );
}

#[test]
fn switching_tabs() {
    let mut sessions = Sessions::default();
    let bid = sessions.open(viewport(), Instant::now());
    sessions.add_tab(bid, tab("a", None), true);
    sessions.add_tab(bid, tab("b", None), false);
    let session = sessions.get_mut(bid).unwrap();
    assert_eq!(session.activate("b"), Some(Some("a".to_owned())));
    assert_eq!(session.activate("nope"), None);
    assert_eq!(session.active_tab().unwrap().id, "b");
    let (owner, found) = sessions.tab_by_devtools_session("S-a").unwrap();
    assert_eq!((owner, found.id.as_str()), (bid, "a"));
}

#[test]
fn frames_are_numbered_and_acknowledged_in_order() {
    let mut sessions = Sessions::default();
    let start = Instant::now();
    let bid = sessions.open(viewport(), start);
    let session = sessions.get_mut(bid).unwrap();
    let first = session.frame_sent("S", 10, start);
    let second = session.frame_sent("S", 11, start + Duration::from_secs(1));
    assert_eq!((first, second), (1, 2));
    assert_eq!(session.in_flight(), 2);

    // An acknowledgement covers the frames sent before it.
    let later = start + Duration::from_secs(2);
    let acked = session.acknowledged(second, later);
    assert_eq!(
        acked.iter().map(|sent| sent.screencast).collect::<Vec<_>>(),
        [10, 11]
    );
    assert_eq!(session.last_ack, later);
    assert!(session.acknowledged(second, later).is_empty());

    let third = session.frame_sent("S", 12, start + Duration::from_secs(3));
    session.frame_sent("S", 13, start + Duration::from_secs(9));
    assert_eq!(third, 3);
    let lost = session.release_sent_before(start + Duration::from_secs(5));
    assert_eq!(lost.len(), 1);
    assert_eq!(session.release_all().len(), 1);
    assert_eq!(session.in_flight(), 0);
}

#[test]
fn an_acknowledgement_covers_frames_already_let_go_and_never_one_not_sent() {
    let mut sessions = Sessions::default();
    let start = Instant::now();
    let bid = sessions.open(viewport(), start);
    let session = sessions.get_mut(bid).unwrap();
    let later = start + Duration::from_secs(1);
    assert!(session.acknowledged(1, later).is_empty());
    assert_eq!(session.last_ack, start);

    for screencast in 10..14 {
        session.frame_sent("S", screencast, start);
    }
    // Frames 1 and 2 were let go when the tab changed; the client skipped 3 and drew 4.
    session.in_flight.drain(..2);
    assert!(session.acknowledged(2, later).is_empty());
    assert_eq!(session.last_ack, later);
    let acked = session.acknowledged(4, later);
    assert_eq!(
        acked.iter().map(|sent| sent.frame).collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(
        session
            .acknowledged(5, start + Duration::from_secs(2))
            .is_empty()
    );
    assert_eq!(session.last_ack, later);
}

#[test]
fn frame_numbers_compare_across_wrapping() {
    assert!(not_after(u32::MAX, 1));
    assert!(!not_after(1, u32::MAX));
    assert!(not_after(7, 7));
}

#[test]
fn quiet_sessions_and_the_last_used() {
    let mut sessions = Sessions::default();
    let start = Instant::now();
    let old = sessions.open(viewport(), start);
    let recent = sessions.open(viewport(), start + Duration::from_secs(60));
    assert_eq!(
        sessions.unacknowledged_since(start + Duration::from_secs(30)),
        [old]
    );
    assert_eq!(sessions.most_recently_used(), Some(recent));
    sessions.get_mut(old).unwrap().last_used = start + Duration::from_secs(90);
    assert_eq!(sessions.most_recently_used(), Some(old));
}
