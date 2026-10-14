//! Byte streams between the relay and the local ports of the previews. The relay opens a stream
//! for every TCP connection a browser needs to a dev server; the app connects to the port on this
//! computer and passes bytes both ways, with flow control so that neither a slow phone nor a fast
//! dev server fills memory on either side.
//!
//! The binary frame that carries a stream's data:
//!
//! ```text
//! byte 0     0x02
//! bytes 1-4  sid, unsigned 32-bit big-endian
//! rest       payload
//! ```

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use futures::future::OptionFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::protocol::{ErrorCode, Frame, RpcError};

pub const STREAM_FRAME_TYPE: u8 = 0x02;
/// Payload one side may have sent on a stream that the other side has not acknowledged.
pub const STREAM_WINDOW_BYTES: usize = 512 * 1024;
/// The largest payload put in one frame.
pub const STREAM_CHUNK_BYTES: usize = 64 * 1024;
/// A receiver acknowledges once this much has been passed on, or when it has passed on all it
/// holds.
pub const STREAM_ACK_BYTES: usize = 32 * 1024;
/// Stream messages waiting for the relay's socket. Streams wait for room beyond this, which keeps
/// what they hold small and lets terminal output and answers go first.
pub const STREAM_QUEUE: usize = 64;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_BYTES: usize = 5;

pub fn encode_stream_frame(sid: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER_BYTES + payload.len());
    frame.push(STREAM_FRAME_TYPE);
    frame.extend_from_slice(&sid.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Reads a frame's sid and payload, or returns `None` when `frame` is not a stream frame.
pub fn decode_stream_frame(frame: &[u8]) -> Option<(u32, &[u8])> {
    let (&[frame_type, a, b, c, d], payload) = frame.split_first_chunk::<HEADER_BYTES>()?;
    (frame_type == STREAM_FRAME_TYPE).then_some((u32::from_be_bytes([a, b, c, d]), payload))
}

/// The sending side of one stream's flow control.
#[derive(Debug, Default)]
pub struct SendWindow {
    in_flight: usize,
}

impl SendWindow {
    /// Bytes that may be sent now.
    pub fn room(&self) -> usize {
        STREAM_WINDOW_BYTES - self.in_flight
    }

    pub fn sent(&mut self, bytes: usize) {
        debug_assert!(bytes <= self.room(), "sent beyond the stream window");
        self.in_flight = (self.in_flight + bytes).min(STREAM_WINDOW_BYTES);
    }

    /// Records an ack. Returns false for an ack of more than is in flight, which is ignored.
    pub fn acked(&mut self, bytes: u64) -> bool {
        match usize::try_from(bytes) {
            Ok(bytes) if bytes <= self.in_flight => {
                self.in_flight -= bytes;
                true
            }
            Ok(_) | Err(_) => false,
        }
    }
}

/// The receiving side: bytes received and not yet acknowledged.
#[derive(Debug, Default)]
pub struct ReceiveWindow {
    held: usize,
    passed: usize,
}

impl ReceiveWindow {
    /// Records bytes that arrived. Returns false when the sender went beyond its window.
    pub fn received(&mut self, bytes: usize) -> bool {
        if self.held + bytes > STREAM_WINDOW_BYTES {
            return false;
        }
        self.held += bytes;
        true
    }

    /// Records bytes passed on, and returns how many to acknowledge now: none until enough has
    /// gathered, unless `drained` says nothing is waiting any more.
    pub fn passed_on(&mut self, bytes: usize, drained: bool) -> usize {
        self.passed = (self.passed + bytes).min(self.held);
        if self.passed == 0 || (self.passed < STREAM_ACK_BYTES && !drained) {
            return 0;
        }
        let ack = self.passed;
        self.held -= ack;
        self.passed = 0;
        ack
    }
}

/// The ports streams may be opened to, by preview id, the main port first. Only previews that
/// are starting or running are in it.
#[derive(Clone, Default)]
pub struct PreviewPorts(Arc<RwLock<HashMap<String, Vec<u16>>>>);

impl PreviewPorts {
    pub fn set(&self, ports: HashMap<String, Vec<u16>>) {
        if let Ok(mut current) = self.0.write() {
            *current = ports;
        }
    }

    /// The port to connect to for `port` of `preview`, the main one when `port` is `None`.
    pub fn lookup(&self, preview: &str, port: Option<u16>) -> Option<u16> {
        let current = self.0.read().ok()?;
        let ports = current.get(preview)?;
        match port {
            Some(port) => ports.contains(&port).then_some(port),
            None => ports.first().copied(),
        }
    }
}

enum Command {
    Data(Vec<u8>),
    Ack(u64),
    /// The relay sends no more data.
    Finish,
}

struct Entry {
    /// Tells this stream apart from a later one with the same sid.
    key: u64,
    commands: mpsc::UnboundedSender<Command>,
    task: JoinHandle<()>,
    relay_closed: bool,
}

/// The open streams of one connection to the relay. Messages to the relay go out through `out`
/// from the streams' own tasks, never from the caller, which is the loop that empties `out`.
/// Dropping the table ends every stream.
pub struct Streams {
    ports: PreviewPorts,
    open: HashMap<u32, Entry>,
    out: mpsc::Sender<Message>,
    gone: mpsc::UnboundedSender<(u32, u64)>,
    next_key: u64,
}

impl Streams {
    /// A table whose streams send through `out`. The receiver yields the sid and key of every
    /// stream that has ended by itself, to be passed to [`Self::ended`].
    pub fn new(
        ports: PreviewPorts,
        out: mpsc::Sender<Message>,
    ) -> (Self, mpsc::UnboundedReceiver<(u32, u64)>) {
        let (gone, ended) = mpsc::unbounded_channel();
        let streams = Self {
            ports,
            open: HashMap::new(),
            out,
            gone,
            next_key: 0,
        };
        (streams, ended)
    }

    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Handles a stream message from the relay. Must be called within a Tokio runtime.
    pub fn handle(&mut self, frame: Frame) {
        match frame {
            Frame::StreamOpen { sid, preview, port } => self.open(sid, &preview, port),
            Frame::StreamClose { sid, error } => self.close(sid, error.is_some()),
            Frame::StreamAck { sid, bytes } => self.send(sid, Command::Ack(bytes)),
            Frame::StreamOpened { .. }
            | Frame::Request { .. }
            | Frame::Response(_)
            | Frame::Notification { .. }
            | Frame::Event { .. }
            | Frame::Ping
            | Frame::Pong => {}
        }
    }

    /// Passes a binary stream frame from the relay on. Returns false for a frame that is not one.
    pub fn data(&mut self, frame: &[u8]) -> bool {
        let Some((sid, payload)) = decode_stream_frame(frame) else {
            return false;
        };
        if self.open.get(&sid).is_some_and(|entry| !entry.relay_closed) {
            self.send(sid, Command::Data(payload.to_vec()));
        }
        true
    }

    /// Forgets a stream that has ended by itself.
    pub fn ended(&mut self, sid: u32, key: u64) {
        if self.open.get(&sid).is_some_and(|entry| entry.key == key) {
            self.open.remove(&sid);
        }
    }

    fn open(&mut self, sid: u32, preview: &str, port: Option<u16>) {
        // The relay only reuses a sid once the stream is gone for it, so an old one is over.
        if let Some(old) = self.open.remove(&sid) {
            old.task.abort();
        }
        let target = self.ports.lookup(preview, port);
        let (commands, received) = mpsc::unbounded_channel();
        self.next_key += 1;
        let key = self.next_key;
        let out = self.out.clone();
        let gone = self.gone.clone();
        let task = tokio::spawn(async move {
            run(sid, target, received, out).await;
            let _ = gone.send((sid, key));
        });
        self.open.insert(
            sid,
            Entry {
                key,
                commands,
                task,
                relay_closed: false,
            },
        );
    }

    fn close(&mut self, sid: u32, failed: bool) {
        if failed {
            if let Some(entry) = self.open.remove(&sid) {
                entry.task.abort();
            }
        } else if let Some(entry) = self.open.get_mut(&sid) {
            entry.relay_closed = true;
            let _ = entry.commands.send(Command::Finish);
        }
    }

    fn send(&self, sid: u32, command: Command) {
        if let Some(entry) = self.open.get(&sid) {
            let _ = entry.commands.send(command);
        }
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        for entry in self.open.values() {
            entry.task.abort();
        }
    }
}

fn text(frame: Frame) -> Message {
    Message::Text(frame.encode())
}

fn closed_with(sid: u32, code: ErrorCode, message: impl Into<String>) -> Message {
    text(Frame::StreamClose {
        sid,
        error: Some(RpcError::new(code, message)),
    })
}

/// Connects to the port on this computer, over IPv6 when nothing listens on IPv4, as dev servers
/// that listen on `localhost` sometimes do.
async fn connect(port: u16) -> io::Result<TcpStream> {
    let ipv4 = timeout(CONNECT_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await;
    let stream = match ipv4 {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
            timeout(CONNECT_TIMEOUT, TcpStream::connect(("::1", port)))
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??
        }
        Ok(Err(error)) => return Err(error),
        Err(_) => return Err(io::ErrorKind::TimedOut.into()),
    };
    stream.set_nodelay(true)?;
    Ok(stream)
}

type Write = Pin<Box<dyn Future<Output = (OwnedWriteHalf, usize, io::Result<()>)> + Send>>;

/// Runs one stream until it is finished both ways or fails. Returns early when the connection's
/// queue is gone.
async fn run(
    sid: u32,
    port: Option<u16>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    out: mpsc::Sender<Message>,
) {
    let Some(port) = port else {
        let _ = out
            .send(closed_with(
                sid,
                ErrorCode::NotFound,
                "No running preview has that port",
            ))
            .await;
        return;
    };
    let socket = match connect(port).await {
        Ok(socket) => socket,
        Err(error) => {
            let message = format!("Nothing answers on port {port}: {error}");
            let _ = out
                .send(closed_with(sid, ErrorCode::Failed, message))
                .await;
            return;
        }
    };
    if out.send(text(Frame::StreamOpened { sid })).await.is_err() {
        return;
    }

    let (mut reader, writer) = socket.into_split();
    let mut buffer = vec![0; STREAM_CHUNK_BYTES];
    let mut send_window = SendWindow::default();
    let mut receive_window = ReceiveWindow::default();
    let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
    let mut idle_writer = Some(writer);
    let mut writing: Option<Write> = None;
    let mut reading = true;
    let mut relay_done = false;
    let mut written_all = false;

    loop {
        if writing.is_none() {
            if let Some(mut writer) = idle_writer.take() {
                if let Some(chunk) = pending.pop_front() {
                    writing = Some(Box::pin(async move {
                        let outcome = writer.write_all(&chunk).await;
                        (writer, chunk.len(), outcome)
                    }));
                } else if relay_done && !written_all {
                    // The dev server reads the end of the request body as end of file.
                    let _ = writer.shutdown().await;
                    written_all = true;
                } else {
                    idle_writer = Some(writer);
                }
            }
        }
        if !reading && written_all {
            return;
        }

        let room = send_window.room().min(STREAM_CHUNK_BYTES);
        tokio::select! {
            Some((writer, bytes, outcome)) = OptionFuture::from(writing.as_mut()), if writing.is_some() => {
                writing = None;
                if let Err(error) = outcome {
                    let message = format!("Could not write to port {port}: {error}");
                    let _ = out.send(closed_with(sid, ErrorCode::Failed, message)).await;
                    return;
                }
                idle_writer = Some(writer);
                if !relay_done {
                    let ack = receive_window.passed_on(bytes, pending.is_empty());
                    if ack > 0 {
                        let ack = Frame::StreamAck { sid, bytes: ack as u64 };
                        if out.send(text(ack)).await.is_err() {
                            return;
                        }
                    }
                }
            }
            command = commands.recv() => match command {
                Some(Command::Data(data)) => {
                    if !receive_window.received(data.len()) {
                        let message = "More data arrived than the stream's window allows";
                        let _ = out.send(closed_with(sid, ErrorCode::Failed, message)).await;
                        return;
                    }
                    pending.push_back(data);
                }
                Some(Command::Ack(bytes)) => {
                    if !send_window.acked(bytes) {
                        log::debug!("[Remote access] stream {sid}: ack beyond what was sent");
                    }
                }
                Some(Command::Finish) => relay_done = true,
                None => return,
            },
            read = reader.read(&mut buffer[..room]), if reading && room > 0 => match read {
                Ok(0) => {
                    reading = false;
                    if out.send(text(Frame::StreamClose { sid, error: None })).await.is_err() {
                        return;
                    }
                }
                Ok(bytes) => {
                    send_window.sent(bytes);
                    let frame = encode_stream_frame(sid, &buffer[..bytes]);
                    if out.send(Message::Binary(frame)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let message = format!("Could not read from port {port}: {error}");
                    let _ = out.send(closed_with(sid, ErrorCode::Failed, message)).await;
                    return;
                }
            },
        }
    }
}

#[cfg(test)]
#[path = "streams_tests.rs"]
mod tests;
