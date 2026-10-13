//! The outbound connection to the relay. It connects with the agent key, keeps the connection
//! alive, reconnects when it drops, and passes frames between the socket and the app.

use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_tungstenite::WebSocketStream;
use async_tungstenite::tokio::{ConnectStream, connect_async_with_tls_connector_and_config};
use async_tungstenite::tungstenite::client::IntoClientRequest;
use async_tungstenite::tungstenite::http::header::AUTHORIZATION;
use async_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use async_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use async_tungstenite::tungstenite::{Error as SocketError, Message};
use futures::{SinkExt, StreamExt};
use rustls_platform_verifier::ConfigVerifierExt;
use serde_json::Value;
use thiserror::Error;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at, sleep, sleep_until, timeout};
use tokio_rustls::TlsConnector;
use url::{Host, Url};

use crate::backoff::Backoff;
use crate::output_frame::{OutputFrame, SessionIdLengthError};
use crate::protocol::{
    CLOSE_REPLACED, Frame, IDLE_TIMEOUT, MAX_FRAME_BYTES, PING_INTERVAL, REPLACED_RECONNECT_DELAY,
    Response, RpcError,
};

const AGENT_PATH: &str = "/ws/agent";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

static TLS_CONFIG: LazyLock<Result<Arc<rustls::ClientConfig>, rustls::Error>> =
    LazyLock::new(|| Ok(Arc::new(rustls::ClientConfig::with_platform_verifier()?)));

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("The server address is not a valid address")]
    InvalidAddress,
    #[error("The server address must start with wss://")]
    Scheme,
    #[error("ws:// is only accepted for a server on this computer; use wss://")]
    InsecureRemote,
    #[error("The access key contains characters that cannot be sent")]
    Key,
}

/// The URL the app connects to on the relay at `server_address`. An address without a scheme
/// is taken to be `wss://`. The unencrypted `ws://` would expose the key on the network, so it is
/// refused for anything but this computer.
pub fn agent_endpoint(server_address: &str) -> Result<String, ConfigError> {
    let address = server_address.trim();
    let url = if address.contains("://") {
        Url::parse(address)
    } else {
        Url::parse(&format!("wss://{address}"))
    }
    .map_err(|_| ConfigError::InvalidAddress)?;

    let scheme = match url.scheme() {
        "wss" | "https" => "wss",
        "ws" | "http" => "ws",
        _ => return Err(ConfigError::Scheme),
    };
    let is_local = match url.host().ok_or(ConfigError::InvalidAddress)? {
        Host::Domain(domain) => domain == "localhost",
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    if scheme == "ws" && !is_local {
        return Err(ConfigError::InsecureRemote);
    }

    let host = url.host_str().ok_or(ConfigError::InvalidAddress)?;
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let prefix = url.path().trim_end_matches('/');
    Ok(format!("{scheme}://{host}{port}{prefix}{AGENT_PATH}"))
}

/// Where to connect and with which key.
pub struct ConnectionConfig {
    endpoint: String,
    authorization: HeaderValue,
}

impl ConnectionConfig {
    pub fn new(server_address: &str, agent_key: &str) -> Result<Self, ConfigError> {
        let endpoint = agent_endpoint(server_address)?;
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {agent_key}")).map_err(|_| ConfigError::Key)?;
        authorization.set_sensitive(true);
        Ok(Self {
            endpoint,
            authorization,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// Not connected; the next attempt follows after `retry_in`.
    Waiting {
        error: String,
        retry_in: Duration,
    },
}

/// What the connection hands to the app, in the order it happened.
pub enum Incoming {
    State(ConnectionState),
    Request {
        method: String,
        params: Value,
        responder: Responder,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// Answers one request. Dropping it leaves the request unanswered, which the relay turns into a
/// timeout for the client.
pub struct Responder {
    id: String,
    sender: OutgoingSender,
}

impl Responder {
    pub fn respond(self, outcome: Result<Value, RpcError>) {
        let frame = Frame::Response(Response::new(self.id, outcome));
        self.sender.send(Message::Text(frame.encode()));
    }
}

struct Outgoing {
    /// The connection the message was written for. A message is never sent on a later one,
    /// where a request id could by then belong to another request.
    epoch: u64,
    message: Message,
}

#[derive(Clone)]
struct OutgoingSender {
    epoch: u64,
    queue: mpsc::UnboundedSender<Outgoing>,
}

impl OutgoingSender {
    /// Returns whether the message was queued.
    fn send(&self, message: Message) -> bool {
        // The receiver is gone only once the connection has been shut down for good.
        self.queue
            .send(Outgoing {
                epoch: self.epoch,
                message,
            })
            .is_ok()
    }
}

/// A running connection to the relay. It reconnects by itself until it is dropped.
pub struct Connection {
    queue: mpsc::UnboundedSender<Outgoing>,
    /// Counts the connections made so far; zero while there is none.
    epoch: Arc<AtomicU64>,
    /// Bytes of terminal output queued and not yet handed to the socket.
    output_backlog: Arc<AtomicUsize>,
    runtime: Option<Runtime>,
}

impl Connection {
    /// Starts connecting on a thread of its own. The receiver yields state changes, requests and
    /// notifications.
    pub fn start(
        config: ConnectionConfig,
    ) -> io::Result<(Self, async_channel::Receiver<Incoming>)> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("hako-remote")
            .enable_io()
            .enable_time()
            .build()?;
        let (queue, outgoing) = mpsc::unbounded_channel();
        let (incoming_sender, incoming) = async_channel::unbounded();
        let epoch = Arc::new(AtomicU64::new(0));
        let output_backlog = Arc::new(AtomicUsize::new(0));
        runtime.spawn(run(
            config,
            epoch.clone(),
            incoming_sender,
            Outbox {
                queue: outgoing,
                output_backlog: output_backlog.clone(),
            },
            queue.clone(),
        ));
        Ok((
            Self {
                queue,
                epoch,
                output_backlog,
                runtime: Some(runtime),
            },
            incoming,
        ))
    }

    /// Sends an event to the relay. Events raised while not connected are dropped. Returns whether
    /// the event was queued.
    pub fn send_event(&self, event: &str, data: Value) -> bool {
        self.send(Message::Text(Frame::event(event, data).encode()))
    }

    /// Sends terminal output to the relay. Output produced while not connected is dropped.
    pub fn send_output(&self, frame: &OutputFrame<'_>) -> Result<(), SessionIdLengthError> {
        let frame = frame.encode()?;
        let length = frame.len();
        // Counted before it is queued, so that the count cannot be taken off first.
        self.output_backlog.fetch_add(length, Ordering::AcqRel);
        if !self.send(Message::Binary(frame)) {
            self.output_backlog.fetch_sub(length, Ordering::AcqRel);
        }
        Ok(())
    }

    /// Bytes of terminal output that wait for the socket, which grows when output is produced
    /// faster than the relay takes it.
    pub fn output_backlog(&self) -> usize {
        self.output_backlog.load(Ordering::Acquire)
    }

    /// Returns whether the message was queued.
    fn send(&self, message: Message) -> bool {
        let epoch = self.epoch.load(Ordering::Acquire);
        epoch != 0
            && OutgoingSender {
                epoch,
                queue: self.queue.clone(),
            }
            .send(message)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            // Dropping a runtime waits for its tasks, which would stall the caller's thread.
            runtime.shutdown_background();
        }
    }
}

type Socket = WebSocketStream<ConnectStream>;

/// The receiving end of the queue of messages to send.
struct Outbox {
    queue: mpsc::UnboundedReceiver<Outgoing>,
    output_backlog: Arc<AtomicUsize>,
}

impl Outbox {
    async fn next(&mut self) -> Option<Outgoing> {
        let outgoing = self.queue.recv().await?;
        match &outgoing.message {
            Message::Binary(frame) => {
                self.output_backlog.fetch_sub(frame.len(), Ordering::AcqRel);
            }
            Message::Text(_)
            | Message::Ping(_)
            | Message::Pong(_)
            | Message::Close(_)
            | Message::Frame(_) => {}
        }
        Some(outgoing)
    }
}

/// Why a connection ended.
enum End {
    /// A newer connection with the same key took its place.
    Replaced,
    Lost(String),
    /// The app no longer listens.
    Stopped,
}

async fn run(
    config: ConnectionConfig,
    epoch: Arc<AtomicU64>,
    incoming: async_channel::Sender<Incoming>,
    mut outgoing: Outbox,
    queue: mpsc::UnboundedSender<Outgoing>,
) {
    let mut backoff = Backoff::new();
    let mut connections = 0;
    let mut failures_since_connected = 0;
    loop {
        if !report(&incoming, ConnectionState::Connecting).await {
            return;
        }
        let (error, retry_in) = match connect(&config).await {
            Ok(socket) => {
                backoff.reset();
                failures_since_connected = 0;
                connections += 1;
                epoch.store(connections, Ordering::Release);
                log::info!("[Remote access] connected to {}", config.endpoint);
                let sender = OutgoingSender {
                    epoch: connections,
                    queue: queue.clone(),
                };
                let end = if report(&incoming, ConnectionState::Connected).await {
                    serve(socket, sender, &incoming, &mut outgoing).await
                } else {
                    End::Stopped
                };
                epoch.store(0, Ordering::Release);
                match end {
                    End::Replaced => (
                        "Another copy of Hako connected with the same access key".to_owned(),
                        REPLACED_RECONNECT_DELAY,
                    ),
                    End::Lost(error) => (error, backoff.next_delay()),
                    End::Stopped => return,
                }
            }
            Err(error) => (error, backoff.next_delay()),
        };

        // A server that stays away is retried for as long as the setting is on, so only the first
        // failure is worth a line in the log.
        failures_since_connected += 1;
        if failures_since_connected == 1 {
            log::warn!(
                "[Remote access] not connected to {}: {error}",
                config.endpoint
            );
        } else {
            log::debug!("[Remote access] still not connected: {error}");
        }
        if !report(&incoming, ConnectionState::Waiting { error, retry_in }).await {
            return;
        }
        sleep(retry_in).await;
    }
}

/// Returns false once the app no longer listens.
async fn report(incoming: &async_channel::Sender<Incoming>, state: ConnectionState) -> bool {
    incoming.send(Incoming::State(state)).await.is_ok()
}

async fn connect(config: &ConnectionConfig) -> Result<Socket, String> {
    let mut request = config
        .endpoint
        .as_str()
        .into_client_request()
        .map_err(|error| error.to_string())?;
    request
        .headers_mut()
        .insert(AUTHORIZATION, config.authorization.clone());
    let connector = if config.endpoint.starts_with("wss://") {
        let tls_config = TLS_CONFIG.clone().map_err(|error| error.to_string())?;
        Some(TlsConnector::from(tls_config))
    } else {
        None
    };
    let socket_config = WebSocketConfig {
        max_message_size: Some(MAX_FRAME_BYTES),
        max_frame_size: Some(MAX_FRAME_BYTES),
        ..Default::default()
    };
    let connecting =
        connect_async_with_tls_connector_and_config(request, connector, Some(socket_config));
    match timeout(CONNECT_TIMEOUT, connecting).await {
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(SocketError::Http(response))) if response.status() == StatusCode::UNAUTHORIZED => {
            Err("The server did not accept the access key".to_owned())
        }
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("The server did not answer in time".to_owned()),
    }
}

async fn serve(
    socket: Socket,
    sender: OutgoingSender,
    incoming: &async_channel::Sender<Incoming>,
    outgoing: &mut Outbox,
) -> End {
    let (mut sink, mut stream) = socket.split();
    let mut ping = interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut idle_deadline = Instant::now() + IDLE_TIMEOUT;
    loop {
        let message = tokio::select! {
            received = stream.next() => {
                let message = match received {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => return End::Lost(error.to_string()),
                    None => return End::Lost("The server closed the connection".to_owned()),
                };
                idle_deadline = Instant::now() + IDLE_TIMEOUT;
                match message {
                    Message::Text(text) => {
                        if !dispatch(&text, &sender, incoming).await {
                            return End::Stopped;
                        }
                    }
                    Message::Close(frame) => return closed(frame),
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
                continue;
            }
            queued = outgoing.next() => match queued {
                Some(queued) if queued.epoch == sender.epoch => queued.message,
                Some(_) => continue,
                None => return End::Stopped,
            },
            _ = ping.tick() => Message::Ping(Vec::new()),
            () = sleep_until(idle_deadline) => {
                return End::Lost("The server has been silent for a minute".to_owned());
            }
        };
        // A peer that stopped reading would otherwise block here and keep the idle check, which
        // only runs between sends, from ever firing.
        match timeout(IDLE_TIMEOUT, sink.send(message)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return End::Lost(error.to_string()),
            Err(_) => return End::Lost("The server stopped reading".to_owned()),
        }
    }
}

fn closed(frame: Option<CloseFrame<'_>>) -> End {
    if frame.is_some_and(|frame| u16::from(frame.code) == CLOSE_REPLACED) {
        End::Replaced
    } else {
        End::Lost("The server closed the connection".to_owned())
    }
}

/// Hands a text frame to the app. Returns false once the app no longer listens.
async fn dispatch(
    text: &str,
    sender: &OutgoingSender,
    incoming: &async_channel::Sender<Incoming>,
) -> bool {
    let frame = match Frame::decode(text) {
        Ok(frame) => frame,
        Err(error) => {
            log::debug!("[Remote access] ignoring a frame that cannot be read: {error}");
            return true;
        }
    };
    let item = match frame {
        Frame::Request { id, method, params } => Incoming::Request {
            method,
            params,
            responder: Responder {
                id,
                sender: sender.clone(),
            },
        },
        Frame::Notification { method, params } => Incoming::Notification { method, params },
        Frame::Response(_) | Frame::Event { .. } | Frame::Ping | Frame::Pong => return true,
    };
    incoming.send(item).await.is_ok()
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
