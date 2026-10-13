//! Live terminals for a remote client. A session that a client subscribes to has what its PTY
//! writes read on the app's main thread a few dozen times a second and sent on as one numbered
//! stream. A terminal nobody watches is not read at all.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::hako_remote::boundary::Boundary;
use ::hako_remote::output_frame::OutputFrame;
use ::hako_remote::output_stream::{MAX_FRAME_PAYLOAD, MAX_HELD_OUTPUT, OutputStream};
use ::hako_remote::prompt_lines::PromptLines;
use ::hako_remote::protocol::{
    EVENT_TERM_CLOSED, EVENT_TERM_SIZE, RpcError, SessionParams, TermInputParams, TermResizeParams,
    TermSize, TermSizeChanged, TermSubscribeResult,
};
use ::hako_remote::{Connection, Responder};
use async_broadcast::{Receiver, TryRecvError};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use parking_lot::FairMutex;
use session_sharing_protocol::common::WindowSize;
use uuid::Uuid;
use warpui::{AppContext, ViewHandle, WeakViewHandle};

use super::handlers::to_json;
use super::snapshot;
use super::terminal_snapshot::terminal_snapshot;
use crate::terminal::TerminalView;
use crate::terminal::model::TerminalModel;

/// How often output is collected and sent while a watched terminal is busy.
pub(super) const PUMP_INTERVAL: Duration = Duration::from_millis(12);
/// How often output is looked for once every watched terminal has been quiet for a while.
const IDLE_PUMP_INTERVAL: Duration = Duration::from_millis(50);
const IDLE_AFTER: Duration = Duration::from_secs(2);
/// How soon after input its echo is looked for, without waiting for the next regular turn.
pub(super) const ECHO_DELAY: Duration = Duration::from_millis(4);

/// How long a subscription to a terminal that was not being read waits to learn whether its
/// output is between two escape sequences.
const JOIN_WAIT: Duration = Duration::from_millis(80);
/// How long a subscription waits for a program to finish a frame it is in the middle of. The
/// terminal itself gives up on such a frame sooner.
const FRAME_WAIT: Duration = Duration::from_millis(250);

/// Output queued for the relay beyond this is not added to. The connection is slower than the
/// terminal then, and a client is better served by a fresh snapshot later than by old output.
const MAX_OUTPUT_BACKLOG: usize = 2 * 1024 * 1024;

/// How long a shell is given to finish being set up before its output is sent on regardless,
/// which is what a shell the app cannot set up needs.
const SETUP_WAIT: Duration = Duration::from_secs(15);

const MIN_SIZE: usize = 2;
const MAX_SIZE: usize = 1000;

/// Whether a session's output is sent on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No subscription has been answered yet, so no client has a place in the output to continue
    /// from.
    Joining,
    /// The shell is being set up. What it prints meanwhile is the setup script, which the app's
    /// own window does not show either.
    SettingUp,
    Live,
}

struct Stream {
    view: WeakViewHandle<TerminalView>,
    model: Arc<FairMutex<TerminalModel>>,
    reads: Receiver<Arc<Vec<u8>>>,
    phase: Phase,
    started: Instant,
    output: OutputStream,
    boundary: Boundary,
    prompt_lines: PromptLines,
    /// Whether the shell prints its prompt as if it took no room. The prompt is then kept on a
    /// line of its own, since the line editor draws the command line from the first column.
    prompt_has_no_width: bool,
    /// The size clients were last told.
    size: TermSize,
    /// Whether a client has set the terminal's size, which then has to be given back.
    sized_by_client: bool,
    last_activity: Instant,
}

/// Where reading a session's PTY output stopped, and after how many bytes.
enum Read {
    /// Everything written so far has been read.
    UpToDate(usize),
    /// The PTY's size was set at this point. What was read was written for the old size, and
    /// there may be more.
    Resized(usize),
    /// The session has ended and everything it wrote has been read.
    Closed,
}

impl Stream {
    fn start(view: &ViewHandle<TerminalView>, seq: u64, ctx: &AppContext) -> Option<Self> {
        let terminal = view.as_ref(ctx);
        Some(Self {
            view: view.downgrade(),
            model: terminal.model.clone(),
            reads: terminal.pty_reads()?,
            phase: Phase::Joining,
            started: Instant::now(),
            output: OutputStream::new(seq, MAX_HELD_OUTPUT),
            boundary: Boundary::new(),
            prompt_lines: PromptLines::default(),
            prompt_has_no_width: false,
            size: size_of(terminal),
            sized_by_client: false,
            last_activity: Instant::now(),
        })
    }

    /// Takes in what the PTY has written since the last time, as far as the next point at which
    /// its size was set.
    fn read(&mut self) -> Read {
        // A full channel turns further reads away, so some may be missing.
        if self.reads.is_full() {
            self.lose_output();
        }
        let mut count = 0;
        let read = loop {
            match self.reads.try_recv() {
                // The PTY's event loop marks a change of size with an empty read.
                Ok(bytes) if bytes.is_empty() => break Read::Resized(count),
                Ok(bytes) => {
                    count += bytes.len();
                    self.boundary.feed(&bytes);
                    match self.phase {
                        Phase::Joining | Phase::SettingUp => continue,
                        Phase::Live => {}
                    }
                    if self.prompt_has_no_width {
                        self.prompt_lines
                            .rewrite(&bytes, |piece| self.output.push(piece));
                    } else {
                        self.output.push(&bytes);
                    }
                }
                Err(TryRecvError::Empty) => break Read::UpToDate(count),
                Err(TryRecvError::Closed) => break Read::Closed,
                Err(TryRecvError::Overflowed(_)) => self.lose_output(),
            }
        };
        if count > 0 {
            self.last_activity = Instant::now();
        }
        read
    }

    /// Takes in everything the PTY has written since the last time. A change of size is
    /// announced in its place: after the output written for the old size, which is sent for
    /// that, and before the rest.
    fn read_all(&mut self, session_id: &str, connection: &Connection, ctx: &AppContext) -> Read {
        let mut total = 0;
        loop {
            match self.read() {
                Read::UpToDate(count) => return Read::UpToDate(total + count),
                Read::Resized(count) => {
                    total += count;
                    self.send_output(session_id, connection);
                    self.announce_size(session_id, connection, ctx);
                }
                Read::Closed => return Read::Closed,
            }
        }
    }

    fn lose_output(&mut self) {
        self.output.mark_lost();
        self.boundary = Boundary::new();
    }

    fn send_output(&mut self, session_id: &str, connection: &Connection) {
        for chunk in self.output.take_chunks(MAX_FRAME_PAYLOAD) {
            let frame = OutputFrame {
                session_id,
                seq: chunk.seq,
                payload: &chunk.payload,
            };
            if let Err(error) = connection.send_output(&frame) {
                log::warn!("[Remote access] terminal output was not sent: {error}");
            }
        }
    }

    /// Tells clients the terminal's size if it is no longer the one they were told.
    fn announce_size(&mut self, session_id: &str, connection: &Connection, ctx: &AppContext) {
        let Some(view) = self.view.upgrade(ctx) else {
            return;
        };
        let size = size_of(view.as_ref(ctx));
        if size == self.size {
            return;
        }
        self.size = size;
        let change = TermSizeChanged {
            session_id: session_id.to_owned(),
            cols: size.cols,
            rows: size.rows,
        };
        if let Ok(data) = to_json(&change) {
            connection.send_event(EVENT_TERM_SIZE, data);
        }
    }

    /// Gives the terminal's size back to the layout of its own window.
    fn release_size(&mut self, ctx: &mut AppContext) {
        if !self.sized_by_client {
            return;
        }
        self.sized_by_client = false;
        if let Some(view) = self.view.upgrade(ctx) {
            view.update(ctx, |view, ctx| view.restore_pty_to_sharer_size(ctx));
        }
    }
}

/// A subscription that is answered once the terminal's output is at a point a snapshot can end
/// on.
struct Subscription {
    session_id: String,
    responder: Responder,
    /// When to stop waiting for such a point and answer anyway.
    deadline: Instant,
}

#[derive(Default)]
pub(super) struct Terminals {
    streams: HashMap<String, Stream>,
    /// Where the output of a session that was streamed before left off, since a session's
    /// offsets never go back.
    offsets: HashMap<String, u64>,
    waiting: Vec<Subscription>,
}

impl Terminals {
    /// How long to wait before collecting output again, or `None` when no terminal is watched.
    pub(super) fn pump_interval(&self) -> Option<Duration> {
        let last_activity = self
            .streams
            .values()
            .map(|stream| stream.last_activity)
            .max()?;
        let is_busy = !self.waiting.is_empty() || last_activity.elapsed() < IDLE_AFTER;
        Some(if is_busy {
            PUMP_INTERVAL
        } else {
            IDLE_PUMP_INTERVAL
        })
    }

    pub(super) fn subscribe(
        &mut self,
        params: SessionParams,
        responder: Responder,
        connection: &Connection,
        ctx: &mut AppContext,
    ) {
        let session_id = params.session_id;
        let Some(view) = find_terminal(&session_id, ctx) else {
            return responder.respond(Err(no_such_terminal()));
        };
        if self.streams.contains_key(&session_id) {
            let subscription = Subscription {
                session_id,
                responder,
                deadline: Instant::now() + FRAME_WAIT,
            };
            if let Some(subscription) = self.answer(subscription, connection, ctx) {
                self.waiting.push(subscription);
            }
            return;
        }

        let seq = self.offsets.get(&session_id).copied().unwrap_or_default();
        let Some(stream) = Stream::start(&view, seq, ctx) else {
            return responder.respond(Err(RpcError::failed(
                "This terminal's output cannot be followed",
            )));
        };
        self.streams.insert(session_id.clone(), stream);
        // Nothing is known yet about where the output stands, so the answer waits for the next
        // time output is collected.
        self.waiting.push(Subscription {
            session_id,
            responder,
            deadline: Instant::now() + JOIN_WAIT,
        });
    }

    /// Answers a subscription with a snapshot, or hands it back when the terminal is in the
    /// middle of something a snapshot cannot express and there is still time to wait.
    fn answer(
        &mut self,
        subscription: Subscription,
        connection: &Connection,
        ctx: &mut AppContext,
    ) -> Option<Subscription> {
        let Some(stream) = self.streams.get_mut(&subscription.session_id) else {
            subscription.responder.respond(Err(no_such_terminal()));
            return None;
        };
        let snapshot = {
            // Reading under the lock leaves nothing in between: the snapshot shows every byte
            // read so far, and none that comes later.
            let model = stream.model.clone();
            let model = model.lock();
            while let Read::Resized(_) = stream.read() {}
            let is_set_up = model.block_list().is_bootstrapped();
            // A shell that is still being set up has an empty snapshot, which is exact anywhere.
            let is_exact = !is_set_up
                || (stream.boundary.is_known() && !model.is_synchronized_output_pending());
            (is_exact || Instant::now() >= subscription.deadline)
                .then(|| (terminal_snapshot(&model), is_set_up))
        };
        let Some((snapshot, is_set_up)) = snapshot else {
            return Some(subscription);
        };
        match (stream.phase, is_set_up) {
            (Phase::Joining, true) => stream.phase = Phase::Live,
            (Phase::Joining, false) => stream.phase = Phase::SettingUp,
            (Phase::SettingUp, true) => {
                // Clients that subscribed during the setup have an empty screen. A gap has them
                // ask for this snapshot too.
                stream.output.mark_lost();
                stream.phase = Phase::Live;
            }
            (Phase::SettingUp, false) | (Phase::Live, true | false) => {}
        }
        if !stream.boundary.is_known() {
            stream.boundary.assume_ground();
        }
        stream.prompt_has_no_width = snapshot.prompt_has_no_width;
        stream
            .prompt_lines
            .set_prompt_above(snapshot.prompt_rows_above);
        // The clients that are already subscribed get what the snapshot covers as output.
        stream.announce_size(&subscription.session_id, connection, ctx);
        stream.send_output(&subscription.session_id, connection);
        stream.last_activity = Instant::now();

        let mut bytes = snapshot.bytes;
        bytes.extend_from_slice(stream.boundary.unfinished());
        subscription
            .responder
            .respond(to_json(&TermSubscribeResult {
                cols: snapshot.cols,
                rows: snapshot.rows,
                seq: stream.output.end(),
                snapshot: BASE64.encode(bytes),
            }));
        None
    }

    pub(super) fn unsubscribe(&mut self, params: SessionParams, ctx: &mut AppContext) {
        // A client whose subscription is about to be answered still needs the stream, even if
        // the last one before it has just left.
        let is_awaited = self
            .waiting
            .iter()
            .any(|subscription| subscription.session_id == params.session_id);
        if !is_awaited {
            self.stop(&params.session_id, ctx);
        }
    }

    fn stop(&mut self, session_id: &str, ctx: &mut AppContext) {
        if let Some(mut stream) = self.streams.remove(session_id) {
            stream.release_size(ctx);
            self.offsets
                .insert(session_id.to_owned(), stream.output.end());
        }
    }

    /// Stops every stream, as when the connection to the relay is lost.
    pub(super) fn stop_all(&mut self, ctx: &mut AppContext) {
        self.waiting.clear();
        let session_ids: Vec<_> = self.streams.keys().cloned().collect();
        for session_id in session_ids {
            self.stop(&session_id, ctx);
        }
    }

    pub(super) fn input(&mut self, params: TermInputParams, ctx: &mut AppContext) {
        let view = match self.streams.get_mut(&params.session_id) {
            Some(stream) => {
                stream.last_activity = Instant::now();
                stream.view.upgrade(ctx)
            }
            None => find_terminal(&params.session_id, ctx),
        };
        if let Some(view) = view {
            view.update(ctx, |view, ctx| {
                view.write_viewer_bytes_to_pty(params.data.into_bytes(), ctx);
            });
        }
    }

    pub(super) fn resize(
        &mut self,
        params: TermResizeParams,
        connection: &Connection,
        ctx: &mut AppContext,
    ) -> Result<TermSize, RpcError> {
        let is_valid = |size| (MIN_SIZE..=MAX_SIZE).contains(&size);
        if !is_valid(params.cols) || !is_valid(params.rows) {
            return Err(RpcError::bad_request(format!(
                "cols and rows must be between {MIN_SIZE} and {MAX_SIZE}"
            )));
        }
        let view = find_terminal(&params.session_id, ctx).ok_or_else(no_such_terminal)?;
        // The size is given back when the last client leaves, which only a terminal that is
        // being watched can notice.
        let Some(stream) = self.streams.get_mut(&params.session_id) else {
            return Ok(size_of(view.as_ref(ctx)));
        };

        // What was written for the old size goes out before the size changes.
        stream.read_all(&params.session_id, connection, ctx);
        stream.send_output(&params.session_id, connection);
        stream.sized_by_client = true;
        view.update(ctx, |view, ctx| {
            view.resize_from_viewer_report(
                WindowSize {
                    num_rows: params.rows,
                    num_cols: params.cols,
                },
                ctx,
            );
        });
        stream.announce_size(&params.session_id, connection, ctx);
        Ok(stream.size)
    }

    pub(super) fn release_size(
        &mut self,
        params: SessionParams,
        connection: &Connection,
        ctx: &mut AppContext,
    ) -> Result<(), RpcError> {
        if find_terminal(&params.session_id, ctx).is_none() {
            return Err(no_such_terminal());
        }
        if let Some(stream) = self.streams.get_mut(&params.session_id) {
            stream.read_all(&params.session_id, connection, ctx);
            stream.send_output(&params.session_id, connection);
            stream.release_size(ctx);
            stream.announce_size(&params.session_id, connection, ctx);
        }
        Ok(())
    }

    /// Collects what the watched terminals have written and sends it on.
    pub(super) fn pump(&mut self, connection: &Connection, ctx: &mut AppContext) {
        let is_congested = connection.output_backlog() > MAX_OUTPUT_BACKLOG;
        let mut closed = Vec::new();
        for (session_id, stream) in &mut self.streams {
            if stream.phase == Phase::SettingUp {
                let is_set_up = stream.model.lock().block_list().is_bootstrapped();
                if is_set_up || stream.started.elapsed() > SETUP_WAIT {
                    // What has been read is setup output as well. A gap has clients ask for a
                    // snapshot, which the shell is now ready for.
                    stream.read_all(session_id, connection, ctx);
                    stream.phase = Phase::Live;
                    stream.lose_output();
                }
            }
            match stream.read_all(session_id, connection, ctx) {
                // Output that has paused is between two pieces.
                Read::UpToDate(0) if !stream.boundary.is_known() => {
                    stream.boundary.assume_ground();
                }
                Read::UpToDate(_) | Read::Resized(_) => {}
                Read::Closed => closed.push(session_id.clone()),
            }
            // A change of size is normally announced where the PTY's output has its mark. One
            // without a mark goes before the output, since a program draws anew after it.
            stream.announce_size(session_id, connection, ctx);
            // While the relay is behind, output is held back, and given up once it is too much.
            if !is_congested {
                stream.send_output(session_id, connection);
            }
        }

        for subscription in std::mem::take(&mut self.waiting) {
            if let Some(subscription) = self.answer(subscription, connection, ctx) {
                self.waiting.push(subscription);
            }
        }
        for session_id in closed {
            self.close(&session_id, connection, ctx);
        }
    }

    /// Ends the streams of sessions whose terminal is no longer in a window.
    pub(super) fn close_ended(&mut self, connection: &Connection, ctx: &mut AppContext) {
        let ended: Vec<_> = self
            .streams
            .keys()
            .filter(|session_id| find_terminal(session_id, ctx).is_none())
            .cloned()
            .collect();
        for session_id in ended {
            self.close(&session_id, connection, ctx);
        }
    }

    fn close(&mut self, session_id: &str, connection: &Connection, ctx: &mut AppContext) {
        if let Some(stream) = self.streams.get_mut(session_id) {
            stream.send_output(session_id, connection);
        }
        self.stop(session_id, ctx);
        connection.send_event(
            EVENT_TERM_CLOSED,
            serde_json::json!({ "session_id": session_id }),
        );
        let (ended, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|subscription| subscription.session_id == session_id);
        self.waiting = waiting;
        for subscription in ended {
            subscription.responder.respond(Err(no_such_terminal()));
        }
    }
}

fn no_such_terminal() -> RpcError {
    RpcError::not_found("No such terminal")
}

fn size_of(terminal: &TerminalView) -> TermSize {
    let size = terminal.size_info();
    TermSize {
        cols: size.columns(),
        rows: size.rows(),
    }
}

/// The terminal of a session, if it is in a tab of some window.
fn find_terminal(session_id: &str, ctx: &AppContext) -> Option<ViewHandle<TerminalView>> {
    let uuid = Uuid::parse_str(session_id).ok()?;
    snapshot::workspace_windows(ctx)
        .into_iter()
        .find_map(|(_, workspace)| {
            workspace.as_ref(ctx).tab_views().find_map(|pane_group| {
                let pane_group = pane_group.as_ref(ctx);
                let pane_id = pane_group.find_terminal_pane_by_session_uuid(uuid.as_bytes())?;
                pane_group.terminal_view_from_pane_id(pane_id, ctx)
            })
        })
}
