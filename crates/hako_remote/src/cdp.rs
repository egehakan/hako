//! A small client of the Chrome DevTools protocol over the browser's WebSocket on the loopback
//! interface: commands with or without waiting for their answer, and the events of every
//! attached target in the order the browser sent them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_tungstenite::tokio::connect_async_with_config;
use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::protocol::WebSocketConfig;
use futures::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

/// Pictures arrive base64-encoded and can be large.
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    /// The attached target's session; `None` for the browser's own events.
    pub session: Option<String>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// Sends commands to the browser. Cloning it gives another handle to the same connection.
#[derive(Clone)]
pub struct Cdp {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl Cdp {
    /// Connects to the browser endpoint `url`. The receiver yields events until the connection
    /// ends.
    pub async fn connect(url: &str) -> Result<(Self, mpsc::UnboundedReceiver<CdpEvent>), String> {
        let config = WebSocketConfig {
            max_message_size: Some(MAX_MESSAGE_BYTES),
            max_frame_size: Some(MAX_MESSAGE_BYTES),
            ..Default::default()
        };
        let (socket, _) = connect_async_with_config(url, Some(config))
            .await
            .map_err(|error| error.to_string())?;
        let (mut sink, mut stream) = socket.split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<String>();
        let (events, received) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let replies = pending.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    message = outgoing.recv() => match message {
                        Some(text) => {
                            if sink.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            let _ = sink.close().await;
                            break;
                        }
                    },
                    received = stream.next() => match received {
                        Some(Ok(Message::Text(text))) => match parse(&text) {
                            Some(Parsed::Reply { id, result }) => {
                                let waiting = replies.lock().ok().and_then(|mut pending| pending.remove(&id));
                                if let Some(waiting) = waiting {
                                    let _ = waiting.send(result);
                                }
                            }
                            Some(Parsed::Event(event)) => {
                                if events.send(event).is_err() {
                                    break;
                                }
                            }
                            None => log::debug!("[Mac browser] ignoring a DevTools message"),
                        },
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        Some(Ok(_)) => {}
                    },
                }
            }
            // Whoever waits for an answer learns that none will come.
            if let Ok(mut pending) = replies.lock() {
                pending.clear();
            }
        });
        Ok((
            Self {
                out,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            received,
        ))
    }

    /// Sends a command whose answer does not matter.
    pub fn send(&self, session: Option<&str>, method: &str, params: Value) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.out.send(encode(id, method, params, session));
    }

    /// Sends a command and waits for its answer.
    pub async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, answer);
        }
        if self.out.send(encode(id, method, params, session)).is_err() {
            return Err("The browser is not connected".to_owned());
        }
        answered
            .await
            .map_err(|_| "The browser closed the connection".to_owned())?
    }
}

pub(crate) fn encode(id: u64, method: &str, params: Value, session: Option<&str>) -> String {
    let mut message = json!({ "id": id, "method": method, "params": params });
    if let Some(session) = session {
        message["sessionId"] = json!(session);
    }
    message.to_string()
}

#[derive(Debug, PartialEq)]
pub(crate) enum Parsed {
    Reply {
        id: u64,
        result: Result<Value, String>,
    },
    Event(CdpEvent),
}

pub(crate) fn parse(text: &str) -> Option<Parsed> {
    let Value::Object(mut message) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let result = match message.remove("error") {
            Some(error) => Err(error
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| error.to_string(), str::to_owned)),
            None => Ok(message
                .remove("result")
                .unwrap_or(Value::Object(Map::new()))),
        };
        return Some(Parsed::Reply { id, result });
    }
    let method = message.remove("method")?.as_str()?.to_owned();
    Some(Parsed::Event(CdpEvent {
        method,
        params: message
            .remove("params")
            .unwrap_or(Value::Object(Map::new())),
        session: message
            .remove("sessionId")
            .and_then(|session| session.as_str().map(str::to_owned)),
    }))
}

#[cfg(test)]
#[path = "cdp_tests.rs"]
mod tests;
