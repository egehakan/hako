//! The viewer sessions of the Mac browser: which tabs each one owns, which of them it watches,
//! and the pictures sent to it that it has not acknowledged yet.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use crate::browser::{BrowserState, TabInfo, Viewport};
use crate::browser_input::InputTranslator;

/// A tab of the browser that a session owns: a DevTools target.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// The target id, which clients know the tab by.
    pub id: String,
    /// The DevTools session attached to it.
    pub devtools_session: String,
    /// The tab that opened it.
    pub opener: Option<String>,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    /// Whether its page is on this computer, where the console is read and pickers reported.
    pub local: bool,
    /// Console messages sent since its page last loaded.
    pub console_sent: usize,
    /// When the newest console message sent was logged, in milliseconds since the epoch.
    pub console_newest: f64,
    /// Messages logged up to then are not sent again: a page restored from the back-forward
    /// cache reports its old ones once more. Set as each new page starts to load.
    pub console_replayed_up_to: f64,
    /// Set up and ready to be watched.
    pub ready: bool,
}

impl Tab {
    pub fn new(id: String, devtools_session: String, opener: Option<String>) -> Self {
        Self {
            id,
            devtools_session,
            opener,
            url: String::new(),
            title: String::new(),
            loading: false,
            can_back: false,
            can_forward: false,
            local: false,
            console_sent: 0,
            console_newest: 0.0,
            console_replayed_up_to: 0.0,
            ready: false,
        }
    }
}

/// A picture sent and not acknowledged yet, and what acknowledges it to the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlight {
    pub frame: u32,
    pub devtools_session: String,
    /// The screencast's own number for the picture.
    pub screencast: i64,
    pub sent: Instant,
}

pub struct Session {
    pub viewport: Viewport,
    tabs: Vec<Tab>,
    active: Option<String>,
    next_frame: u32,
    in_flight: VecDeque<InFlight>,
    /// When the client last acknowledged a picture, or opened the session.
    pub last_ack: Instant,
    /// When a picture was last sent.
    pub last_frame: Instant,
    /// When the client last did anything with the session.
    pub last_used: Instant,
    pub input: InputTranslator,
}

impl Session {
    fn new(viewport: Viewport, now: Instant) -> Self {
        Self {
            viewport,
            tabs: Vec::new(),
            active: None,
            next_frame: 1,
            in_flight: VecDeque::new(),
            last_ack: now,
            last_frame: now,
            last_used: now,
            input: InputTranslator::default(),
        }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        let active = self.active.as_deref()?;
        self.tabs.iter().find(|tab| tab.id == active)
    }

    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    /// Makes `id` the tab whose pictures are sent. Returns the tab that was active before, or
    /// `None` when the session has no such tab.
    pub fn activate(&mut self, id: &str) -> Option<Option<String>> {
        self.tab(id)?;
        Some(self.active.replace(id.to_owned()))
    }

    /// Numbers a picture about to be sent and keeps it until it is acknowledged.
    pub fn frame_sent(&mut self, devtools_session: &str, screencast: i64, now: Instant) -> u32 {
        let frame = self.next_frame;
        self.next_frame = self.next_frame.wrapping_add(1).max(1);
        self.in_flight.push_back(InFlight {
            frame,
            devtools_session: devtools_session.to_owned(),
            screencast,
            sent: now,
        });
        self.last_frame = now;
        frame
    }

    /// Takes the pictures that the client's acknowledgement of `frame` covers: it and every one
    /// sent before it.
    pub fn acknowledged(&mut self, frame: u32, now: Instant) -> Vec<InFlight> {
        let Some(position) = self.in_flight.iter().position(|sent| sent.frame == frame) else {
            return Vec::new();
        };
        self.last_ack = now;
        self.in_flight.drain(..=position).collect()
    }

    /// Takes every picture in flight, as when they can no longer reach the client.
    pub fn release_all(&mut self) -> Vec<InFlight> {
        self.in_flight.drain(..).collect()
    }

    /// Takes the pictures sent before `before`, which were lost on the way.
    pub fn release_sent_before(&mut self, before: Instant) -> Vec<InFlight> {
        let lost = self
            .in_flight
            .iter()
            .take_while(|sent| sent.sent < before)
            .count();
        self.in_flight.drain(..lost).collect()
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    pub fn tab_infos(&self) -> Vec<TabInfo> {
        self.tabs
            .iter()
            .map(|tab| TabInfo {
                id: tab.id.clone(),
                url: tab.url.clone(),
                title: tab.title.clone(),
                active: self.active.as_deref() == Some(tab.id.as_str()),
            })
            .collect()
    }

    pub fn state(&self, bid: u32) -> BrowserState {
        let active = self.active_tab();
        BrowserState {
            bid,
            tab: active.map(|tab| tab.id.clone()),
            url: active.map(|tab| tab.url.clone()).unwrap_or_default(),
            title: active.map(|tab| tab.title.clone()).unwrap_or_default(),
            loading: active.is_some_and(|tab| tab.loading),
            can_back: active.is_some_and(|tab| tab.can_back),
            can_forward: active.is_some_and(|tab| tab.can_forward),
            tabs: self.tab_infos(),
        }
    }
}

/// What closing a tab did to its session.
#[derive(Clone, Debug, PartialEq)]
pub struct Removed {
    pub bid: u32,
    pub tab: Tab,
    /// The tab that became active in its place, when it was the active one.
    pub activated: Option<String>,
    /// The session has no tabs left.
    pub empty: bool,
}

#[derive(Default)]
pub struct Sessions {
    sessions: BTreeMap<u32, Session>,
    next_bid: u32,
}

impl Sessions {
    pub fn open(&mut self, viewport: Viewport, now: Instant) -> u32 {
        loop {
            self.next_bid = self.next_bid.wrapping_add(1).max(1);
            if !self.sessions.contains_key(&self.next_bid) {
                break;
            }
        }
        self.sessions
            .insert(self.next_bid, Session::new(viewport, now));
        self.next_bid
    }

    pub fn close(&mut self, bid: u32) -> Option<Session> {
        self.sessions.remove(&bid)
    }

    pub fn get(&self, bid: u32) -> Option<&Session> {
        self.sessions.get(&bid)
    }

    pub fn get_mut(&mut self, bid: u32) -> Option<&mut Session> {
        self.sessions.get_mut(&bid)
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn bids(&self) -> Vec<u32> {
        self.sessions.keys().copied().collect()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut Session)> {
        self.sessions
            .iter_mut()
            .map(|(bid, session)| (*bid, session))
    }

    /// Adds a tab to a session, as its active one when `activate` says so or when it has none.
    /// Returns false when there is no such session.
    pub fn add_tab(&mut self, bid: u32, tab: Tab, activate: bool) -> bool {
        let Some(session) = self.sessions.get_mut(&bid) else {
            return false;
        };
        if activate || session.active.is_none() {
            session.active = Some(tab.id.clone());
        }
        session.tabs.retain(|existing| existing.id != tab.id);
        session.tabs.push(tab);
        true
    }

    /// The session that owns the tab `id`.
    pub fn owner(&self, id: &str) -> Option<u32> {
        self.sessions
            .iter()
            .find(|(_, session)| session.tab(id).is_some())
            .map(|(bid, _)| *bid)
    }

    pub fn tab_mut(&mut self, id: &str) -> Option<(u32, &mut Tab)> {
        self.sessions.iter_mut().find_map(|(bid, session)| {
            session
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id)
                .map(|tab| (*bid, tab))
        })
    }

    pub fn tab_by_devtools_session(&mut self, devtools_session: &str) -> Option<(u32, &mut Tab)> {
        self.sessions.iter_mut().find_map(|(bid, session)| {
            session
                .tabs
                .iter_mut()
                .find(|tab| tab.devtools_session == devtools_session)
                .map(|tab| (*bid, tab))
        })
    }

    /// Whether `id` is the tab a session watches.
    pub fn is_active(&self, id: &str) -> bool {
        self.sessions
            .values()
            .any(|session| session.active.as_deref() == Some(id))
    }

    /// Forgets a tab that has closed. The tab that opened it takes its place as the active one
    /// when the session still has it, and otherwise the newest tab does.
    pub fn remove_tab(&mut self, id: &str) -> Option<Removed> {
        let (bid, session) = self
            .sessions
            .iter_mut()
            .find(|(_, session)| session.tab(id).is_some())?;
        let position = session.tabs.iter().position(|tab| tab.id == id)?;
        let tab = session.tabs.remove(position);
        let mut activated = None;
        if session.active.as_deref() == Some(id) {
            let opener = tab
                .opener
                .as_deref()
                .filter(|opener| session.tab(opener).is_some());
            let next = opener
                .map(str::to_owned)
                .or_else(|| session.tabs.last().map(|tab| tab.id.clone()));
            session.active = next.clone();
            activated = next;
        }
        Some(Removed {
            bid: *bid,
            tab,
            activated,
            empty: session.tabs.is_empty(),
        })
    }

    /// The session the client used last, which a tab whose opener is unknown goes to.
    pub fn most_recently_used(&self) -> Option<u32> {
        self.sessions
            .iter()
            .max_by_key(|(_, session)| session.last_used)
            .map(|(bid, _)| *bid)
    }

    /// The sessions whose client has acknowledged nothing since `since`.
    pub fn unacknowledged_since(&self, since: Instant) -> Vec<u32> {
        self.sessions
            .iter()
            .filter(|(_, session)| session.last_ack < since)
            .map(|(bid, _)| *bid)
            .collect()
    }
}

#[cfg(test)]
#[path = "browser_sessions_tests.rs"]
mod tests;
