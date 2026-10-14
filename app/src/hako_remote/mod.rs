//! Remote access. While the setting is on, the app keeps a connection to the relay, answers what
//! a remote client asks about its windows and tabs, says when they change, and lets the client
//! add a folder, watch and use a terminal, read, change, download and upload files, make a
//! directory, create, rename, copy, move, trash, zip and unzip files, and use source control in the repositories under the folders. It also tells the
//! owner's devices when a terminal wants them, and keeps the computer awake while it is on.
//! Previews run whether or not it is on; the Mac browser that shows them runs on the connection.

mod app_info;
mod handlers;
mod keep_awake;
mod notifications;
mod previews;
mod snapshot;
mod terminal;
mod terminal_snapshot;

use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ::hako_remote::file_ops::{self, CopyKind, Deadline};
use ::hako_remote::git::{Announce, ChangeLimiter, SourceControl, may_have_changed};
use ::hako_remote::protocol::{
    EVENT_GIT_CHANGED, EVENT_NOTIFY, EVENT_WORKSPACE_CHANGED, FolderCreateParams, FsArchiveParams,
    FsCopyParams, FsDownloadParams, FsExtractParams, FsPathParams, FsRenameParams, FsTrashParams,
    FsUploadBeginParams, FsUploadChunkParams, FsUploadIdParams, FsWriteParams, GitChanged,
    GitCommitParams, GitDiffParams, GitDiscardParams, GitLogParams, GitMergeParams, GitRepoParams,
    GitShowParams, GitSwitchParams, Method, NOTIFICATION_TERM_INPUT, Notify, RpcError, Workspace,
};
use ::hako_remote::transfers::{self, Transfers, UNFINISHED_KEPT_FOR};
use ::hako_remote::{
    BrowserConfig, Connection, ConnectionConfig, ConnectionState, Incoming, Responder, archive,
    extract, files, uploads_area,
};
use futures::future::BoxFuture;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use settings::Setting as _;
use warpui::r#async::{FutureExt as _, Timer};
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity, WindowId};

use self::handlers::{NewFolder, NewTab, parse, to_json};
use self::keep_awake::KeepAwake;
pub(crate) use self::notifications::FinishedCommand;
use self::notifications::Notifications;
use self::previews::{PreviewChange, Previews};
use self::terminal::{ECHO_DELAY, PUMP_INTERVAL, Terminals};
use crate::code::file_tree::trash::move_to_trash;
use crate::settings::RemoteAccessSettings;
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;

const KEY_FILE_NAME: &str = "remote.key";

/// How often the workspace is compared with what clients were last told, which also caps how
/// often they are told that it changed.
const WORKSPACE_CHECK_INTERVAL: Duration = Duration::from_millis(500);
const KEY_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// How often, and how many times, a window opened for a request is looked at for what the
/// request asked for.
const NEW_WINDOW_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const NEW_WINDOW_CHECKS: usize = 50;

/// How long a source control request waits to learn the `PATH` of the owner's shell before git
/// is run with the app's own.
const SHELL_PATH_WAIT: Duration = Duration::from_secs(10);

/// How often the uploads area is cleaned while the app runs, which keeps it at least daily even
/// when the computer's sleep holds the timer back.
const UPLOADS_AREA_CLEAN_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How often the archives that have been kept long enough are looked for.
const ARCHIVES_CLEAN_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteAccessStatus {
    Off,
    /// On, but there is no access key to connect with.
    MissingKey,
    /// On, but the server address or the key cannot be used.
    Misconfigured {
        error: String,
    },
    Connecting {
        /// Why the last attempt, or the last connection, ended.
        last_error: Option<String>,
    },
    Connected,
}

pub enum HakoRemoteEvent {
    StatusChanged,
}

/// What the connection is made from: the settings and the key on disk.
#[derive(PartialEq, Eq)]
struct Target {
    server_address: String,
    key: Option<String>,
}

pub struct HakoRemote {
    connection: Option<Connection>,
    /// What the current state was set up for; `None` while remote access is off.
    target: Option<Target>,
    /// Bumped whenever the connection is replaced, so that what an earlier one still delivers
    /// is ignored.
    generation: u64,
    /// Bumped whenever the periodic check is restarted or stopped, so that only one runs.
    check: u64,
    status: RemoteAccessStatus,
    /// The workspace as clients were last told about.
    announced: Option<Workspace>,
    terminals: Terminals,
    /// Bumped whenever the next collection of terminal output is scheduled, so that only the
    /// latest one runs.
    pump: u64,
    source_control: Arc<SourceControl>,
    git_changes: ChangeLimiter,
    notifications: Notifications,
    keep_awake: KeepAwake,
    transfers: Arc<Transfers>,
    previews: Previews,
}

/// What a source control request is answered with.
struct GitRequest {
    source_control: Arc<SourceControl>,
    /// The directories of the folders the repositories are looked for in.
    folders: Vec<String>,
    /// The `PATH` of the owner's shell, when it could be learned.
    path_env: Option<String>,
}

impl HakoRemote {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(&RemoteAccessSettings::handle(ctx), |me, _, _, ctx| {
            me.apply_settings(ctx);
        });
        ctx.subscribe_to_model(&CLIAgentSessionsModel::handle(ctx), |me, _, event, ctx| {
            let notify = me.notifications.agent_event(event, ctx);
            me.deliver(notify);
        });
        let mut me = Self {
            connection: None,
            target: None,
            generation: 0,
            check: 0,
            status: RemoteAccessStatus::Off,
            announced: None,
            terminals: Terminals::default(),
            pump: 0,
            source_control: Arc::new(SourceControl::new(handlers::working_trees)),
            git_changes: ChangeLimiter::default(),
            notifications: Notifications::default(),
            keep_awake: KeepAwake::default(),
            transfers: Arc::new(Transfers::new(warp_core::paths::data_dir())),
            previews: Previews::default(),
        };
        let transfers = me.transfers.clone();
        ctx.spawn(
            async move {
                transfers.start(SystemTime::now(), &uploads_area::today());
                archive::clean(&Self::archives_area(), SystemTime::now(), Duration::ZERO);
            },
            |me, (), ctx| {
                me.schedule_uploads_area_clean(ctx);
                Self::schedule_archives_clean(ctx);
            },
        );
        me.apply_settings(ctx);
        Self::start_previews(ctx);
        me
    }

    /// Removes the temporary files of the unfinished uploads, as the app quits.
    pub fn abandon_uploads(&self) {
        self.transfers.abandon_all();
    }

    /// Quits the Mac browser's Chrome, as the app quits.
    pub fn quit_browser(&self) {
        if let Some(connection) = &self.connection {
            connection.quit_browser();
        }
    }

    fn schedule_uploads_area_clean(&self, ctx: &mut ModelContext<Self>) {
        let transfers = self.transfers.clone();
        ctx.spawn(
            async move {
                Timer::after(UPLOADS_AREA_CLEAN_INTERVAL).await;
                transfers.clean_uploads_area(SystemTime::now(), &uploads_area::today());
            },
            |me, (), ctx| me.schedule_uploads_area_clean(ctx),
        );
    }

    fn schedule_archives_clean(ctx: &mut ModelContext<Self>) {
        ctx.spawn(
            async move {
                Timer::after(ARCHIVES_CLEAN_INTERVAL).await;
                archive::clean(&Self::archives_area(), SystemTime::now(), archive::KEPT_FOR);
            },
            |_, (), ctx| Self::schedule_archives_clean(ctx),
        );
    }

    /// Where folders are zipped to be downloaded.
    fn archives_area() -> PathBuf {
        warp_core::paths::data_dir().join(archive::ARCHIVES_AREA)
    }

    pub fn status(&self) -> &RemoteAccessStatus {
        &self.status
    }

    /// The file the access key is read from.
    pub fn key_path() -> PathBuf {
        warp_core::paths::data_dir().join(KEY_FILE_NAME)
    }

    fn read_key() -> Option<String> {
        let key = fs::read_to_string(Self::key_path()).ok()?;
        let key = key.trim();
        (!key.is_empty()).then(|| key.to_owned())
    }

    /// Brings the connection in line with the settings and the key file.
    fn apply_settings(&mut self, ctx: &mut ModelContext<Self>) {
        let settings = RemoteAccessSettings::as_ref(ctx);
        let target = (*settings.enabled).then(|| Target {
            server_address: settings.server_address.value().clone(),
            key: Self::read_key(),
        });
        if target == self.target {
            return;
        }

        self.connection = None;
        self.generation += 1;
        self.check += 1;
        self.announced = None;
        self.notifications.pending.clear();
        self.stop_terminals(ctx);
        self.target = target;
        let status = match &self.target {
            None => RemoteAccessStatus::Off,
            Some(Target { key: None, .. }) => RemoteAccessStatus::MissingKey,
            Some(Target {
                server_address,
                key: Some(key),
            }) => {
                let (server_address, key) = (server_address.clone(), key.clone());
                match self.connect(&server_address, &key, ctx) {
                    Ok(()) => RemoteAccessStatus::Connecting { last_error: None },
                    Err(error) => RemoteAccessStatus::Misconfigured { error },
                }
            }
        };
        self.set_status(status, ctx);
        if self.status == RemoteAccessStatus::MissingKey {
            self.schedule_check(ctx);
        }
    }

    fn connect(
        &mut self,
        server_address: &str,
        key: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), String> {
        let config =
            ConnectionConfig::new(server_address, key).map_err(|error| error.to_string())?;
        let browser = BrowserConfig {
            data_dir: warp_core::paths::data_dir(),
            previews: self.previews.addresses.clone(),
        };
        let (connection, incoming) =
            Connection::start(config, self.previews.ports.clone(), browser).map_err(|error| {
                log::error!("[Remote access] could not start the connection thread: {error}");
                "The connection could not be started".to_owned()
            })?;
        self.connection = Some(connection);
        let generation = self.generation;
        ctx.spawn_stream_local(
            incoming,
            move |me, incoming, ctx| {
                if me.generation == generation {
                    me.handle_incoming(incoming, ctx);
                }
            },
            |_, _| {},
        );
        Ok(())
    }

    /// Notes that a command has started in the terminal of `terminal_view_id`.
    pub(crate) fn command_started(&mut self, terminal_view_id: EntityId) {
        self.notifications
            .command_started(terminal_view_id, Instant::now());
    }

    /// How long the command that has just ended in the terminal of `terminal_view_id` ran: the
    /// duration its block measured, or else the time since it started.
    pub(crate) fn command_duration(
        &self,
        terminal_view_id: EntityId,
        block_duration: Option<Duration>,
        finished_at: Instant,
    ) -> Option<Duration> {
        block_duration.or_else(|| {
            self.notifications
                .command_started_at(terminal_view_id)
                .map(|started_at| finished_at.saturating_duration_since(started_at))
        })
    }

    /// Tells the owner's devices that a command that ran for long in the terminal of
    /// `terminal_view_id` has ended.
    pub(crate) fn command_finished(
        &mut self,
        terminal_view_id: EntityId,
        command: FinishedCommand,
        ctx: &mut ModelContext<Self>,
    ) {
        // The terminal reports this in the middle of its own update, when its tab's title cannot
        // be read yet.
        ctx.spawn(futures::future::ready(()), move |me, (), ctx| {
            let notify = me
                .notifications
                .command_finished(terminal_view_id, command, ctx);
            me.deliver(notify);
        });
    }

    /// Sends a notification now, or keeps it for a while when the relay is out of reach.
    fn deliver(&mut self, notify: Option<Notify>) {
        let Some(notify) = notify else {
            return;
        };
        match &self.status {
            RemoteAccessStatus::Connected => {
                let sent = self
                    .connection
                    .as_ref()
                    .is_some_and(|connection| connection.send_event(EVENT_NOTIFY, json!(notify)));
                if !sent {
                    // The connection has dropped and the app has not heard yet.
                    self.notifications.pending.keep(notify, Instant::now());
                }
            }
            RemoteAccessStatus::Connecting { .. } => {
                self.notifications.pending.keep(notify, Instant::now());
            }
            RemoteAccessStatus::Off
            | RemoteAccessStatus::MissingKey
            | RemoteAccessStatus::Misconfigured { .. } => {}
        }
    }

    fn handle_incoming(&mut self, incoming: Incoming, ctx: &mut ModelContext<Self>) {
        match incoming {
            Incoming::State(state) => self.connection_state_changed(state, ctx),
            Incoming::Request {
                method,
                params,
                responder,
            } => self.handle_request(&method, params, responder, ctx),
            Incoming::Notification { method, params } => {
                self.handle_notification(&method, params, ctx);
            }
        }
    }

    fn handle_request(
        &mut self,
        method: &str,
        params: Value,
        responder: Responder,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(known_method) = Method::from_name(method) else {
            return responder.respond(Err(RpcError::not_supported(method)));
        };
        // A request arrives over the connection, so there is one for as long as it is handled.
        let Some(connection) = &self.connection else {
            return;
        };
        let done = |outcome: Result<(), RpcError>| outcome.map(|()| json!({}));
        let outcome = match known_method {
            Method::AppInfo => to_json(&app_info::app_info()),
            Method::WorkspaceList => to_json(&snapshot::workspace(ctx)),
            Method::TabCreate => {
                match parse(params).and_then(|params| handlers::create_tab(params, ctx)) {
                    Ok(NewTab::Open(tab)) => to_json(&tab),
                    Ok(NewTab::InNewWindow(window_id)) => {
                        return self.answer_from_new_window(
                            window_id,
                            responder,
                            NEW_WINDOW_CHECKS,
                            "The window opened without a terminal",
                            ctx,
                            |window_id, ctx| handlers::first_tab(window_id, ctx),
                        );
                    }
                    Err(error) => Err(error),
                }
            }
            Method::TabActivate => {
                done(parse(params).and_then(|params| handlers::activate_tab(params, ctx)))
            }
            Method::TabClose => {
                done(parse(params).and_then(|params| handlers::close_tab(params, ctx)))
            }
            Method::TabRename => {
                done(parse(params).and_then(|params| handlers::rename_tab(params, ctx)))
            }
            Method::FolderCreate => return Self::create_folder(params, responder, ctx),
            Method::FolderRename => {
                done(parse(params).and_then(|params| handlers::rename_folder(params, ctx)))
            }
            Method::TermSubscribe => {
                return match parse(params) {
                    Ok(params) => {
                        self.terminals.subscribe(params, responder, connection, ctx);
                        self.schedule_pump(PUMP_INTERVAL, ctx);
                    }
                    Err(error) => responder.respond(Err(error)),
                };
            }
            Method::TermUnsubscribe => {
                done(parse(params).map(|params| self.terminals.unsubscribe(params, ctx)))
            }
            Method::TermResize => parse(params)
                .and_then(|params| self.terminals.resize(params, connection, ctx))
                .and_then(|size| to_json(&size)),
            Method::TermReleaseSize => done(
                parse(params)
                    .and_then(|params| self.terminals.release_size(params, connection, ctx)),
            ),
            Method::ThemeGet => to_json(&handlers::theme(ctx)),
            Method::FsRoots => to_json(&handlers::file_roots(ctx)),
            Method::FsList => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsPathParams| files::list(&params.path),
                );
            }
            Method::FsRead => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsPathParams| files::read(&params.path),
                );
            }
            Method::FsWrite => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsWriteParams| files::write(&params),
                );
            }
            Method::FsMkdir => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsPathParams| files::mkdir(&params.path).map(|()| json!({})),
                );
            }
            Method::FsStat => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsPathParams| file_ops::stat(&params.path),
                );
            }
            Method::FsCreateFile => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsPathParams| file_ops::create_file(&params.path),
                );
            }
            Method::FsRename => {
                let home = dirs::home_dir().unwrap_or_default();
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    move |params: FsRenameParams| file_ops::rename(&params, &home),
                );
            }
            Method::FsCopy | Method::FsMove => {
                let kind = if known_method == Method::FsCopy {
                    CopyKind::Copy
                } else {
                    CopyKind::Move
                };
                let deadline = Deadline::from_now();
                return Self::answer_on_own_thread(
                    params,
                    responder,
                    ctx,
                    move |params: FsCopyParams| file_ops::copy_or_move(&params, kind, &deadline),
                );
            }
            Method::FsTrash => {
                let home = dirs::home_dir().unwrap_or_default();
                let folders = handlers::folder_directories(ctx);
                return Self::answer_on_own_thread(
                    params,
                    responder,
                    ctx,
                    move |params: FsTrashParams| {
                        file_ops::trash(&params, &home, &folders, |path| {
                            move_to_trash(path).map_err(|error| error.to_string())
                        })
                    },
                );
            }
            Method::FsArchive => {
                let deadline = Deadline::from_now();
                return Self::answer_on_own_thread(
                    params,
                    responder,
                    ctx,
                    move |params: FsArchiveParams| {
                        archive::create(
                            &params,
                            &Self::archives_area(),
                            transfers::MAX_BYTES,
                            &deadline,
                        )
                    },
                );
            }
            Method::FsExtract => {
                let deadline = Deadline::from_now();
                return Self::answer_on_own_thread(
                    params,
                    responder,
                    ctx,
                    move |params: FsExtractParams| {
                        extract::extract(&params, extract::LIMITS, &deadline)
                    },
                );
            }
            Method::FsTransferInfo => to_json(&transfers::info()),
            Method::FsDownload => {
                return Self::answer_in_background(
                    params,
                    responder,
                    ctx,
                    |params: FsDownloadParams| transfers::download(&params),
                );
            }
            Method::FsUploadBegin => {
                return self.answer_upload(
                    params,
                    responder,
                    ctx,
                    |transfers, params: FsUploadBeginParams| {
                        transfers.begin(params, Instant::now(), &uploads_area::today())
                    },
                );
            }
            Method::FsUploadChunk => {
                return self.answer_upload(
                    params,
                    responder,
                    ctx,
                    |transfers, params: FsUploadChunkParams| {
                        transfers.chunk(params, Instant::now())
                    },
                );
            }
            Method::FsUploadEnd => {
                return self.answer_upload(
                    params,
                    responder,
                    ctx,
                    |transfers, params: FsUploadIdParams| transfers.end(params, Instant::now()),
                );
            }
            Method::FsUploadCancel => {
                return self.answer_upload(
                    params,
                    responder,
                    ctx,
                    |transfers, params: FsUploadIdParams| {
                        transfers.cancel(params, Instant::now()).map(|()| json!({}))
                    },
                );
            }
            Method::GitRepos => {
                return self.answer_source_control(
                    params,
                    responder,
                    ctx,
                    |git, _: Value| async move {
                        Ok(git.source_control.repos(&git.folders, git.path_env).await)
                    },
                );
            }
            Method::GitStatus => {
                return self.answer_source_control(
                    params,
                    responder,
                    ctx,
                    |git, params: GitRepoParams| async move {
                        git.source_control
                            .status(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitDiff => {
                return self.answer_source_control(
                    params,
                    responder,
                    ctx,
                    |git, params: GitDiffParams| async move {
                        git.source_control
                            .diff(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitCommit => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitCommitParams| async move {
                        git.source_control
                            .commit(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitPush => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitRepoParams| async move {
                        git.source_control
                            .push(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitPull => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitRepoParams| async move {
                        git.source_control
                            .pull(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitDiscard => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitDiscardParams| async move {
                        git.source_control
                            .discard(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitBranches => {
                return self.answer_source_control(
                    params,
                    responder,
                    ctx,
                    |git, params: GitRepoParams| async move {
                        git.source_control
                            .branches(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitSwitch => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitSwitchParams| async move {
                        git.source_control
                            .switch(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitLog => {
                return self.answer_source_control(
                    params,
                    responder,
                    ctx,
                    |git, params: GitLogParams| async move {
                        git.source_control
                            .log(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitShow => {
                return self.answer_source_control(
                    params,
                    responder,
                    ctx,
                    |git, params: GitShowParams| async move {
                        git.source_control
                            .show(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::GitMerge => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitMergeParams| async move {
                        git.source_control
                            .merge(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
            Method::PreviewList => to_json(&self.preview_list()),
            Method::PreviewStop => done(self.change_preview(params, PreviewChange::Stop, ctx)),
            Method::PreviewRestart => {
                done(self.change_preview(params, PreviewChange::Restart, ctx))
            }
            Method::PreviewRemove => done(self.change_preview(params, PreviewChange::Remove, ctx)),
            Method::GitMergeAbort => {
                return self.change_repository(
                    params,
                    responder,
                    ctx,
                    |git, params: GitRepoParams| async move {
                        git.source_control
                            .merge_abort(&git.folders, &params, git.path_env)
                            .await
                    },
                );
            }
        };
        responder.respond(outcome);
    }

    /// Answers a request with what `work` makes of its params. The work is done off the app's
    /// main thread, which must not wait for the file system.
    fn answer_in_background<P, R, W>(
        params: Value,
        responder: Responder,
        ctx: &mut ModelContext<Self>,
        work: W,
    ) where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        W: FnOnce(P) -> Result<R, RpcError> + Send + 'static,
    {
        ctx.spawn(
            async move {
                let outcome = parse(params)
                    .and_then(work)
                    .and_then(|result| to_json(&result));
                responder.respond(outcome);
            },
            |_, (), _| {},
        );
    }

    /// Answers a request with what `work` makes of its params, on a thread of its own: the work
    /// can take minutes, which must hold up neither the app's main thread nor other requests.
    fn answer_on_own_thread<P, R, W>(
        params: Value,
        responder: Responder,
        ctx: &mut ModelContext<Self>,
        work: W,
    ) where
        P: DeserializeOwned + Send + 'static,
        R: Serialize + Send + 'static,
        W: FnOnce(P) -> Result<R, RpcError> + Send + 'static,
    {
        ctx.spawn(
            async move {
                let outcome = match parse(params) {
                    Ok(params) => file_ops::on_own_thread(move || work(params))
                        .await
                        .and_then(|outcome| outcome),
                    Err(error) => Err(error),
                };
                responder.respond(outcome.and_then(|result| to_json(&result)));
            },
            |_, (), _| {},
        );
    }

    /// Answers `folder.create`. The directory is examined off the app's main thread, which must
    /// not wait for the file system, and the folder is added on it.
    fn create_folder(params: Value, responder: Responder, ctx: &mut ModelContext<Self>) {
        ctx.spawn(
            async move {
                let params: FolderCreateParams = parse(params)?;
                files::readable_directory(&params.directory)?;
                Ok::<_, RpcError>(params)
            },
            |me, params, ctx| match params.and_then(|params| handlers::create_folder(params, ctx)) {
                Ok(NewFolder::Added(folder)) => responder.respond(to_json(&folder)),
                Ok(NewFolder::InNewWindow(window_id, directory)) => me.answer_from_new_window(
                    window_id,
                    responder,
                    NEW_WINDOW_CHECKS,
                    "The window opened without its tab list",
                    ctx,
                    move |window_id, ctx| handlers::folder_in_window(window_id, &directory, ctx),
                ),
                Err(error) => responder.respond(Err(error)),
            },
        );
    }

    /// Answers a request about an upload off the app's main thread, and gives up on uploads that
    /// have been left unfinished for long once that is due.
    fn answer_upload<P, R>(
        &self,
        params: Value,
        responder: Responder,
        ctx: &mut ModelContext<Self>,
        work: fn(&Transfers, P) -> Result<R, RpcError>,
    ) where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
    {
        let transfers = self.transfers.clone();
        Self::answer_in_background(params, responder, ctx, move |params| {
            work(&transfers, params)
        });
        let transfers = self.transfers.clone();
        ctx.spawn(
            async move {
                Timer::after(UNFINISHED_KEPT_FOR + Duration::from_secs(1)).await;
                transfers.expire(Instant::now());
            },
            |_, (), _| {},
        );
    }

    /// Answers a request with what `work` learns from git, which it waits for off the app's
    /// main thread.
    fn answer_source_control<P, R, W, F>(
        &self,
        params: Value,
        responder: Responder,
        ctx: &mut ModelContext<Self>,
        work: W,
    ) where
        P: DeserializeOwned + Send + 'static,
        R: Serialize + 'static,
        W: FnOnce(GitRequest, P) -> F + Send + 'static,
        F: Future<Output = Result<R, RpcError>> + Send,
    {
        let request = self.git_request(ctx);
        ctx.spawn(
            async move {
                let outcome = match parse(params) {
                    Ok(params) => work(request.await, params).await,
                    Err(error) => Err(error),
                };
                responder.respond(outcome.and_then(|result| to_json(&result)));
            },
            |_, (), _| {},
        );
    }

    /// Answers a request that changes the repository its params name, and tells clients
    /// afterwards that the repository may have changed.
    fn change_repository<P, R, W, F>(
        &self,
        params: Value,
        responder: Responder,
        ctx: &mut ModelContext<Self>,
        work: W,
    ) where
        P: DeserializeOwned + Send + 'static,
        R: Serialize + 'static,
        W: FnOnce(GitRequest, P) -> F + Send + 'static,
        F: Future<Output = Result<R, RpcError>> + Send,
    {
        let repo = params
            .get("repo")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let request = self.git_request(ctx);
        ctx.spawn(
            async move {
                let outcome = match parse(params) {
                    Ok(params) => work(request.await, params).await,
                    Err(error) => Err(error),
                };
                let may_have_changed = may_have_changed(&outcome);
                responder.respond(outcome.and_then(|result| to_json(&result)));
                may_have_changed
            },
            |me, may_have_changed, ctx| {
                if let Some(repo) = repo.filter(|_| may_have_changed) {
                    me.announce_git_change(repo, ctx);
                }
            },
        );
    }

    /// Gathers what a source control request is answered with. Only the folders need the
    /// app's main thread.
    fn git_request(
        &self,
        ctx: &mut ModelContext<Self>,
    ) -> impl Future<Output = GitRequest> + use<> {
        let source_control = self.source_control.clone();
        let folders = handlers::folder_directories(ctx);
        let shell_path = Self::shell_path(ctx);
        async move {
            GitRequest {
                source_control,
                folders,
                path_env: shell_path
                    .with_timeout(SHELL_PATH_WAIT)
                    .await
                    .ok()
                    .flatten(),
            }
        }
    }

    /// The `PATH` of the owner's login shell, once it is known.
    fn shell_path(ctx: &mut ModelContext<Self>) -> BoxFuture<'static, Option<String>> {
        #[cfg(feature = "local_tty")]
        {
            use crate::terminal::local_shell::LocalShellState;
            LocalShellState::handle(ctx).update(ctx, |shell_state, ctx| {
                shell_state.get_interactive_path_env_var(ctx)
            })
        }
        #[cfg(not(feature = "local_tty"))]
        {
            let _ = ctx;
            Box::pin(futures::future::ready(None))
        }
    }

    /// Tells clients that a repository may have changed, now or, when they were told so a
    /// moment ago, as soon as they may be told again.
    fn announce_git_change(&mut self, repo: String, ctx: &mut ModelContext<Self>) {
        match self.git_changes.changed(&repo) {
            Announce::Now => self.send_git_changed(repo),
            Announce::After(delay) => {
                ctx.spawn(
                    async move {
                        Timer::after(delay).await;
                    },
                    move |me, _, _| {
                        me.git_changes.announced_late(&repo);
                        me.send_git_changed(repo);
                    },
                );
            }
            Announce::Covered => {}
        }
    }

    fn send_git_changed(&self, repo: String) {
        if let Some(connection) = &self.connection {
            let changed = GitChanged { repo: Some(repo) };
            connection.send_event(EVENT_GIT_CHANGED, json!(changed));
        }
    }

    fn handle_notification(&mut self, method: &str, params: Value, ctx: &mut ModelContext<Self>) {
        if method != NOTIFICATION_TERM_INPUT {
            log::debug!("[Remote access] ignoring the notification {method}");
            return;
        }
        match parse(params) {
            Ok(params) => {
                self.terminals.input(params, ctx);
                if self.terminals.pump_interval().is_some() {
                    self.schedule_pump(ECHO_DELAY, ctx);
                }
            }
            Err(error) => log::debug!("[Remote access] ignoring input: {}", error.message),
        }
    }

    /// Answers a request with what `look` finds in a window that was opened for it, looking
    /// again in a moment while it finds nothing, and with `missing` when it never does.
    fn answer_from_new_window<R, L>(
        &mut self,
        window_id: WindowId,
        responder: Responder,
        checks_left: usize,
        missing: &'static str,
        ctx: &mut ModelContext<Self>,
        look: L,
    ) where
        R: Serialize,
        L: Fn(WindowId, &mut AppContext) -> Option<R> + 'static,
    {
        if let Some(found) = look(window_id, ctx) {
            return responder.respond(to_json(&found));
        }
        let Some(checks_left) = checks_left.checked_sub(1) else {
            return responder.respond(Err(RpcError::failed(missing)));
        };
        ctx.spawn(
            async move {
                Timer::after(NEW_WINDOW_CHECK_INTERVAL).await;
            },
            move |me, _, ctx| {
                me.answer_from_new_window(window_id, responder, checks_left, missing, ctx, look);
            },
        );
    }

    /// Collects the output of the watched terminals after `delay`, and from then on for as long
    /// as any is watched.
    fn schedule_pump(&mut self, delay: Duration, ctx: &mut ModelContext<Self>) {
        self.pump += 1;
        let pump = self.pump;
        ctx.spawn(
            async move {
                Timer::after(delay).await;
            },
            move |me, _, ctx| {
                if me.pump == pump {
                    me.pump_terminals(ctx);
                }
            },
        );
    }

    fn pump_terminals(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        self.terminals.pump(connection, ctx);
        if let Some(interval) = self.terminals.pump_interval() {
            self.schedule_pump(interval, ctx);
        }
    }

    /// Stops following every terminal, which also gives back the size of those a client had
    /// resized.
    fn stop_terminals(&mut self, ctx: &mut ModelContext<Self>) {
        self.pump += 1;
        self.terminals.stop_all(ctx);
    }

    fn connection_state_changed(&mut self, state: ConnectionState, ctx: &mut ModelContext<Self>) {
        let status = match state {
            ConnectionState::Connecting => match &self.status {
                RemoteAccessStatus::Connecting { .. } => return,
                RemoteAccessStatus::Off
                | RemoteAccessStatus::MissingKey
                | RemoteAccessStatus::Misconfigured { .. }
                | RemoteAccessStatus::Connected => {
                    self.stop_terminals(ctx);
                    RemoteAccessStatus::Connecting { last_error: None }
                }
            },
            ConnectionState::Connected => {
                // Clients ask for the workspace when they see the app come online, so only what
                // changes from here on is news to them.
                self.announced = Some(snapshot::workspace(ctx));
                if let Some(connection) = &self.connection {
                    for notify in self.notifications.pending.take(Instant::now()) {
                        connection.send_event(EVENT_NOTIFY, json!(notify));
                    }
                }
                RemoteAccessStatus::Connected
            }
            ConnectionState::Waiting { error, retry_in: _ } => {
                // The relay forgets every subscription with the connection, and clients
                // subscribe again when the app is back.
                self.stop_terminals(ctx);
                // The key may be what was wrong, so a key file that has changed since is put to
                // use right away.
                let generation = self.generation;
                self.apply_settings(ctx);
                if self.generation != generation {
                    return;
                }
                RemoteAccessStatus::Connecting {
                    last_error: Some(error),
                }
            }
        };
        self.set_status(status, ctx);
        self.check += 1;
        if self.status == RemoteAccessStatus::Connected {
            self.schedule_check(ctx);
        }
    }

    fn set_status(&mut self, status: RemoteAccessStatus, ctx: &mut ModelContext<Self>) {
        if self.status != status {
            self.status = status;
            self.keep_awake.set(matches!(
                self.status,
                RemoteAccessStatus::Connecting { .. } | RemoteAccessStatus::Connected
            ));
            ctx.emit(HakoRemoteEvent::StatusChanged);
            // Whether previews can be opened from elsewhere is in the state file.
            self.previews_changed(ctx);
        }
    }

    /// Runs [`Self::periodic_check`] after the interval that suits the current state.
    fn schedule_check(&mut self, ctx: &mut ModelContext<Self>) {
        let check = self.check;
        let interval = match &self.status {
            RemoteAccessStatus::MissingKey => KEY_CHECK_INTERVAL,
            RemoteAccessStatus::Off
            | RemoteAccessStatus::Misconfigured { .. }
            | RemoteAccessStatus::Connecting { .. }
            | RemoteAccessStatus::Connected => WORKSPACE_CHECK_INTERVAL,
        };
        ctx.spawn(
            async move {
                Timer::after(interval).await;
            },
            move |me, _, ctx| {
                if me.check == check {
                    me.periodic_check(ctx);
                }
            },
        );
    }

    fn periodic_check(&mut self, ctx: &mut ModelContext<Self>) {
        match &self.status {
            RemoteAccessStatus::MissingKey => {
                // Picks up a key file added after remote access was turned on.
                self.apply_settings(ctx);
                if self.status == RemoteAccessStatus::MissingKey {
                    self.schedule_check(ctx);
                }
            }
            RemoteAccessStatus::Connected => {
                self.announce_workspace_change(ctx);
                if let Some(connection) = &self.connection {
                    self.terminals.close_ended(connection, ctx);
                }
                self.schedule_check(ctx);
            }
            RemoteAccessStatus::Off
            | RemoteAccessStatus::Misconfigured { .. }
            | RemoteAccessStatus::Connecting { .. } => {}
        }
    }

    /// Tells clients to fetch the workspace again when it is no longer what they were last told.
    fn announce_workspace_change(&mut self, ctx: &mut ModelContext<Self>) {
        let workspace = snapshot::workspace(ctx);
        if self.announced.as_ref() == Some(&workspace) {
            return;
        }
        self.announced = Some(workspace);
        if let Some(connection) = &self.connection {
            connection.send_event(EVENT_WORKSPACE_CHANGED, json!({}));
        }
    }
}

impl Entity for HakoRemote {
    type Event = HakoRemoteEvent;
}

impl SingletonEntity for HakoRemote {}
