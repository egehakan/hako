//! Previews: dev servers that tools such as the preview skill start through request files, run
//! in tabs of their own like any other command, and that the phone opens through the relay. They
//! run whether or not remote access is on; only opening them from elsewhere needs it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::hako_remote::file_ops;
use ::hako_remote::notify::one_line;
use ::hako_remote::previews::{
    Check, Checked, PREVIEWS_DIR, Preview, PreviewIdParams, PreviewList, PreviewRequest,
    PreviewStatus, PreviewsState, REQUESTS_DIR, Registry, RequestFile, STATE_FILE, ShellSyntax,
    TabWork, answers, command_line, read_state, take_requests, write_state,
};
use ::hako_remote::protocol::{
    EVENT_PREVIEWS_CHANGED, NOTIFY_TITLE_MAX_CHARS, Notify, NotifyKind, RpcError, TabCreateParams,
    TabParams, TabRenameParams,
};
use ::hako_remote::rename::MAX_TAB_TITLE_CHARS;
use ::hako_remote::streams::PreviewPorts;
use serde_json::{Value, json};
use warpui::r#async::Timer;
use warpui::{EntityId, ModelContext, WindowId};

use super::handlers::{self, NewTab, parse};
use super::terminal::find_terminal;
use super::{HakoRemote, RemoteAccessStatus, snapshot};
use crate::terminal::shell::ShellType;

/// How often the previews that wait for something are looked at.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const REQUESTS_INTERVAL: Duration = Duration::from_secs(1);
/// `previews.changed` is sent at most this often.
const ANNOUNCE_INTERVAL: Duration = Duration::from_millis(500);

const REMOTE_ACCESS_OFF: &str =
    "Remote access is off on this Mac, so the preview opens only here, not from the phone";

#[derive(Default)]
pub(super) struct Previews {
    registry: Registry,
    /// The registry has been read from the state file.
    loaded: bool,
    /// Shared with the connection, which opens streams to these ports.
    pub(super) ports: PreviewPorts,
    /// The terminal of each preview's tab, by preview id, to tell whose command has ended.
    terminals: HashMap<String, EntityId>,
    /// Previews whose tab is the first one of a window that is opening.
    opening: HashMap<String, WindowId>,
    /// Previews whose check is under way.
    checking: HashSet<String>,
    last_announced: Option<Instant>,
    announce_scheduled: bool,
    /// The latest state handed to be written, and the latest written, so that an older write that
    /// finishes late does not replace a newer one.
    state_version: u64,
    written_version: Arc<Mutex<u64>>,
}

fn previews_dir() -> PathBuf {
    warp_core::paths::data_dir().join(PREVIEWS_DIR)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

impl HakoRemote {
    /// Reads the previews kept from the last run and starts following the request files.
    pub(super) fn start_previews(ctx: &mut ModelContext<Self>) {
        ctx.spawn(
            async move {
                let directory = previews_dir();
                if let Err(error) = std::fs::create_dir_all(directory.join(REQUESTS_DIR)) {
                    log::warn!(
                        "[Previews] could not create {}: {error}",
                        directory.display()
                    );
                }
                read_state(&directory.join(STATE_FILE))
            },
            |me, state, ctx| {
                me.previews.registry = Registry::restore(state, Instant::now());
                me.previews.loaded = true;
                me.previews_changed(ctx);
                me.schedule_preview_requests(ctx);
                me.schedule_preview_tick(ctx);
            },
        );
    }

    fn schedule_preview_requests(&self, ctx: &mut ModelContext<Self>) {
        ctx.spawn(
            async move {
                Timer::after(REQUESTS_INTERVAL).await;
                take_requests(&previews_dir().join(REQUESTS_DIR))
            },
            |me, files, ctx| {
                me.handle_preview_requests(files, ctx);
                me.schedule_preview_requests(ctx);
            },
        );
    }

    fn schedule_preview_tick(&self, ctx: &mut ModelContext<Self>) {
        ctx.spawn(
            async move {
                Timer::after(TICK_INTERVAL).await;
            },
            |me, (), ctx| {
                me.preview_tick(ctx);
                me.schedule_preview_tick(ctx);
            },
        );
    }

    fn handle_preview_requests(&mut self, files: Vec<RequestFile>, ctx: &mut ModelContext<Self>) {
        for RequestFile { file, request } in files {
            let now = Instant::now();
            let registry = &mut self.previews.registry;
            let outcome = match request {
                Ok(PreviewRequest::Start(request)) => Ok(registry.start(request, now_ms(), now)),
                Ok(PreviewRequest::Attach(request)) => Ok(registry.attach(request, now_ms(), now)),
                Ok(PreviewRequest::Stop(params)) => registry
                    .stop(&params.id, now)
                    .map_err(|e| (Some(params.id), e)),
                Ok(PreviewRequest::Restart(params)) => registry
                    .restart(&params.id, now_ms(), now)
                    .map_err(|e| (Some(params.id), e)),
                Ok(PreviewRequest::Remove(params)) => registry
                    .remove(&params.id)
                    .map_err(|e| (Some(params.id), e)),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(outcome) => {
                    self.do_tab_work(outcome.work, ctx);
                    self.previews_changed(ctx);
                }
                Err((id, message)) => {
                    self.previews.registry.reject(file, id, message, now_ms());
                    self.previews_changed(ctx);
                }
            }
        }
    }

    /// Answers `preview.list`.
    pub(super) fn preview_list(&self) -> PreviewList {
        let mut previews = self.previews.registry.list();
        self.mark_remote_access_off(previews.iter_mut());
        PreviewList { previews }
    }

    /// Answers `preview.stop`, `preview.restart` and `preview.remove`.
    pub(super) fn change_preview(
        &mut self,
        params: Value,
        change: PreviewChange,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RpcError> {
        let PreviewIdParams { id } = parse(params)?;
        let id = id.as_str();
        let now = Instant::now();
        let registry = &mut self.previews.registry;
        let outcome = match change {
            PreviewChange::Stop => registry.stop(id, now),
            PreviewChange::Restart => registry.restart(id, now_ms(), now),
            PreviewChange::Remove => registry.remove(id),
        }
        .map_err(RpcError::not_found)?;
        self.do_tab_work(outcome.work, ctx);
        self.previews_changed(ctx);
        Ok(())
    }

    /// Notes that a command has ended in the terminal of `terminal_view_id`.
    pub(crate) fn command_ended(
        &mut self,
        terminal_view_id: EntityId,
        exit_status: i32,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(id) = self
            .previews
            .terminals
            .iter()
            .find_map(|(id, view_id)| (*view_id == terminal_view_id).then(|| id.clone()))
        else {
            return;
        };
        if self
            .previews
            .registry
            .command_ended(&id, exit_status, Instant::now())
        {
            self.previews_changed(ctx);
        }
    }

    fn do_tab_work(&mut self, work: Vec<TabWork>, ctx: &mut ModelContext<Self>) {
        for work in work {
            match work {
                TabWork::Interrupt { tab_id } => {
                    let session_id = self
                        .previews
                        .registry
                        .with_tab(&tab_id)
                        .and_then(|preview| preview.session_id.clone());
                    if let Some(view) = session_id.and_then(|id| find_terminal(&id, ctx)) {
                        view.update(ctx, |view, ctx| {
                            view.write_viewer_bytes_to_pty(vec![0x03], ctx);
                        });
                    }
                }
                TabWork::Close { tab_id } => {
                    if let Err(error) = handlers::close_tab(TabParams { tab_id }, ctx) {
                        log::info!("[Previews] the tab was not closed: {}", error.message);
                    }
                }
            }
        }
    }

    fn preview_tick(&mut self, ctx: &mut ModelContext<Self>) {
        let now = Instant::now();
        let mut changed = false;
        for (id, tab_id) in self.previews.registry.tabs() {
            if !handlers::tab_exists(&tab_id, ctx) {
                self.previews.terminals.remove(&id);
                changed |= self.previews.registry.tab_closed(&id, now);
            }
        }
        for (id, tab_id) in self.previews.registry.launching() {
            changed |= self.launch_preview(&id, tab_id, ctx);
        }
        changed |= self.previews.registry.expire_launches(now);
        if changed {
            self.previews_changed(ctx);
        }

        for check in self.previews.registry.due_checks(now) {
            if self.previews.checking.insert(check.id.clone()) {
                self.run_preview_check(check, ctx);
            }
        }
    }

    fn run_preview_check(&self, check: Check, ctx: &mut ModelContext<Self>) {
        let (port, path, wait) = (check.port, check.path.clone(), check.wait);
        ctx.spawn(
            file_ops::on_own_thread(move || answers(port, &path, wait)),
            move |me, answered, ctx| {
                me.previews.checking.remove(&check.id);
                let answered = answered.unwrap_or(false);
                match me
                    .previews
                    .registry
                    .checked(&check, answered, Instant::now())
                {
                    Checked::Unchanged => {}
                    Checked::Changed => me.previews_changed(ctx),
                    Checked::Ready => {
                        me.notify_preview_ready(&check.id);
                        me.previews_changed(ctx);
                    }
                }
            },
        );
    }

    /// Gets a preview's command run in its tab: opens the tab if it has none, and runs the
    /// command once its shell is ready for it. Returns whether the preview changed.
    fn launch_preview(
        &mut self,
        id: &str,
        tab_id: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let now = Instant::now();
        let Some(stored) = self.previews.registry.get(id).cloned() else {
            return false;
        };
        let preview = &stored.preview;
        if tab_id.is_none() {
            let title = one_line(&format!("Preview: {}", preview.name), MAX_TAB_TITLE_CHARS);
            if let Some((tab_id, session_id)) = self.restored_preview_tab(&title, ctx) {
                return self.previews.registry.set_tab(id, tab_id, session_id);
            }
            let tab = match self.previews.opening.get(id) {
                Some(window_id) => match handlers::first_tab(*window_id, ctx) {
                    Some(tab) => tab,
                    None => return false,
                },
                None => {
                    let params = TabCreateParams {
                        window_id: None,
                        folder_id: None,
                        directory: Some(preview.directory.clone()),
                    };
                    match handlers::create_tab(params, ctx) {
                        Ok(NewTab::Open(tab)) => tab,
                        Ok(NewTab::InNewWindow(window_id)) => {
                            self.previews.opening.insert(id.to_owned(), window_id);
                            return false;
                        }
                        Err(error) => {
                            let message = format!(
                                "No tab could be opened in {}: {}",
                                preview.directory, error.message
                            );
                            return self.previews.registry.launch_failed(id, &message, now);
                        }
                    }
                }
            };
            self.previews.opening.remove(id);
            let rename = TabRenameParams {
                tab_id: tab.tab_id.clone(),
                title: Some(title),
            };
            if let Err(error) = handlers::rename_tab(rename, ctx) {
                log::info!("[Previews] the tab was not named: {}", error.message);
            }
            return self
                .previews
                .registry
                .set_tab(id, tab.tab_id, tab.session_id);
        };

        let Some(command) = &preview.command else {
            return false;
        };
        let Some(view) = preview
            .session_id
            .as_deref()
            .and_then(|session_id| find_terminal(session_id, ctx))
        else {
            return false;
        };
        let is_ready = {
            let model = view.as_ref(ctx).model.clone();
            let model = model.lock();
            let block_list = model.block_list();
            block_list.is_bootstrapped() && !block_list.active_block().is_executing()
        };
        if !is_ready {
            return false;
        }
        let syntax = match view.as_ref(ctx).active_session_shell_type(ctx) {
            Some(ShellType::Fish) => ShellSyntax::Fish,
            Some(ShellType::Zsh | ShellType::Bash | ShellType::PowerShell) | None => {
                ShellSyntax::Posix
            }
        };
        let line = command_line(&preview.directory, command, &stored.env, syntax);
        self.previews.terminals.insert(id.to_owned(), view.id());
        // Run as if entered in the terminal's input, which shows it as the tab's command. Bytes
        // typed straight into the shell race the input report the app asks the shell for, whose
        // key sequence then lands in the server's output.
        view.update(ctx, |view, ctx| {
            view.execute_command_or_set_pending(&line, ctx);
        });
        self.previews.registry.launched(id, now);
        false
    }

    /// A tab titled `title` that no preview runs in, with its terminal: the tab of a preview
    /// that the app brought back when it was started again.
    fn restored_preview_tab(
        &self,
        title: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Option<(String, String)> {
        snapshot::workspace(ctx)
            .windows
            .into_iter()
            .flat_map(|window| window.tabs)
            .filter(|tab| tab.title == title && self.previews.registry.with_tab(&tab.id).is_none())
            .find_map(|tab| {
                let session_id = tab.panes.into_iter().find_map(|pane| pane.session_id)?;
                Some((tab.id, session_id))
            })
    }

    fn notify_preview_ready(&mut self, id: &str) {
        let Some(preview) = self.previews.registry.get(id).map(|stored| &stored.preview) else {
            return;
        };
        let Some(session_id) = preview.session_id.clone() else {
            return;
        };
        let notify = Notify {
            kind: NotifyKind::PreviewReady,
            title: one_line(
                &format!("Preview ready: {}", preview.name),
                NOTIFY_TITLE_MAX_CHARS,
            ),
            body: preview.directory.clone(),
            session_id,
            tab_id: preview.tab_id.clone(),
            preview_id: Some(preview.id.clone()),
        };
        self.deliver(Some(notify));
    }

    /// Says on previews that are starting or running when nothing outside this Mac can open them.
    fn mark_remote_access_off<'a>(&self, previews: impl Iterator<Item = &'a mut Preview>) {
        let is_off = match &self.status {
            RemoteAccessStatus::Off
            | RemoteAccessStatus::MissingKey
            | RemoteAccessStatus::Misconfigured { .. } => true,
            RemoteAccessStatus::Connecting { .. } | RemoteAccessStatus::Connected => false,
        };
        if !is_off {
            return;
        }
        for preview in previews {
            match preview.status {
                PreviewStatus::Starting | PreviewStatus::Running => {
                    preview
                        .message
                        .get_or_insert_with(|| REMOTE_ACCESS_OFF.to_owned());
                }
                PreviewStatus::Stopped | PreviewStatus::Failed => {}
            }
        }
    }

    /// Passes a change of the previews on: to the streams, the state file and clients.
    pub(super) fn previews_changed(&mut self, ctx: &mut ModelContext<Self>) {
        if !self.previews.loaded {
            return;
        }
        self.previews.ports.set(self.previews.registry.open_ports());
        self.write_previews_state(ctx);
        self.announce_previews(ctx);
    }

    fn write_previews_state(&mut self, ctx: &mut ModelContext<Self>) {
        let mut state: PreviewsState = self.previews.registry.state(now_ms());
        self.mark_remote_access_off(state.previews.iter_mut().map(|stored| &mut stored.preview));
        self.previews.state_version += 1;
        let version = self.previews.state_version;
        let written_version = self.previews.written_version.clone();
        ctx.spawn(
            async move {
                let Ok(mut written) = written_version.lock() else {
                    return;
                };
                if *written >= version {
                    return;
                }
                let path = previews_dir().join(STATE_FILE);
                match write_state(&path, &state) {
                    Ok(()) => *written = version,
                    Err(error) => log::warn!("[Previews] could not write the state: {error}"),
                }
            },
            |_, (), _| {},
        );
    }

    fn announce_previews(&mut self, ctx: &mut ModelContext<Self>) {
        if self.previews.announce_scheduled {
            return;
        }
        let wait = self
            .previews
            .last_announced
            .map(|last| ANNOUNCE_INTERVAL.saturating_sub(last.elapsed()))
            .unwrap_or_default();
        if wait.is_zero() {
            self.send_previews_changed();
            return;
        }
        self.previews.announce_scheduled = true;
        ctx.spawn(
            async move {
                Timer::after(wait).await;
            },
            |me, (), _| {
                me.previews.announce_scheduled = false;
                me.send_previews_changed();
            },
        );
    }

    fn send_previews_changed(&mut self) {
        self.previews.last_announced = Some(Instant::now());
        if let (RemoteAccessStatus::Connected, Some(connection)) = (&self.status, &self.connection)
        {
            connection.send_event(EVENT_PREVIEWS_CHANGED, json!({}));
        }
    }
}

/// What a client asks of a preview.
pub(super) enum PreviewChange {
    Stop,
    Restart,
    Remove,
}
