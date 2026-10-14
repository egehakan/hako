use std::sync::mpsc as std_mpsc;

use async_tungstenite::tokio::accept_hdr_async;
use async_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use serde_json::json;
use tokio::net::TcpListener;

use super::*;
use crate::browser::PreviewAddresses;
use crate::protocol::EVENT_WORKSPACE_CHANGED;

#[test]
fn appends_the_agent_path_to_a_secure_address() {
    assert_eq!(
        agent_endpoint("wss://hako.egehakan.com").as_deref(),
        Ok("wss://hako.egehakan.com/ws/agent")
    );
}

#[test]
fn takes_an_address_without_a_scheme_to_be_secure() {
    assert_eq!(
        agent_endpoint(" hako.egehakan.com/ ").as_deref(),
        Ok("wss://hako.egehakan.com/ws/agent")
    );
}

#[test]
fn accepts_an_https_address() {
    assert_eq!(
        agent_endpoint("https://hako.egehakan.com:8443/relay/").as_deref(),
        Ok("wss://hako.egehakan.com:8443/relay/ws/agent")
    );
}

#[test]
fn accepts_an_unencrypted_address_on_this_computer() {
    assert_eq!(
        agent_endpoint("ws://localhost:8788").as_deref(),
        Ok("ws://localhost:8788/ws/agent")
    );
    assert_eq!(
        agent_endpoint("ws://127.0.0.1:8788").as_deref(),
        Ok("ws://127.0.0.1:8788/ws/agent")
    );
    assert_eq!(
        agent_endpoint("ws://[::1]:8788").as_deref(),
        Ok("ws://[::1]:8788/ws/agent")
    );
}

#[test]
fn refuses_an_unencrypted_address_elsewhere() {
    assert_eq!(
        agent_endpoint("ws://hako.egehakan.com"),
        Err(ConfigError::InsecureRemote)
    );
}

#[test]
fn refuses_an_address_of_another_scheme() {
    assert_eq!(
        agent_endpoint("ftp://hako.egehakan.com"),
        Err(ConfigError::Scheme)
    );
}

#[test]
fn refuses_an_empty_address() {
    assert_eq!(agent_endpoint(""), Err(ConfigError::InvalidAddress));
}

#[test]
fn refuses_a_key_that_cannot_be_a_header() {
    assert_eq!(
        ConnectionConfig::new("ws://localhost:1", "line\nbreak").err(),
        Some(ConfigError::Key)
    );
}

/// Plays the relay for one connection: reports the `Authorization` header it was sent, sends
/// `request`, then reports the next two text frames it receives.
async fn relay_once(listener: TcpListener, request: String, seen: std_mpsc::Sender<String>) {
    let (stream, _) = listener.accept().await.unwrap();
    let header_seen = seen.clone();
    let mut socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
        let authorization = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        header_seen.send(authorization.to_owned()).unwrap();
        Ok::<_, ErrorResponse>(response)
    })
    .await
    .unwrap();
    socket.send(Message::Text(request)).await.unwrap();
    let mut received = 0;
    while received < 2 {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            seen.send(text).unwrap();
            received += 1;
        }
    }
}

#[test]
fn connects_with_the_key_then_answers_a_request_and_sends_an_event() {
    let relay = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_io()
        .build()
        .unwrap();
    let listener = relay.block_on(TcpListener::bind(("127.0.0.1", 0))).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_sender, seen) = std_mpsc::channel();
    let request = r#"{"t":"req","id":"c1:7","method":"app.info"}"#.to_owned();
    relay.spawn(relay_once(listener, request, seen_sender));

    let config = ConnectionConfig::new(&format!("ws://127.0.0.1:{port}"), "secret-key").unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let browser = BrowserConfig {
        data_dir: data_dir.path().to_owned(),
        previews: PreviewAddresses::default(),
    };
    let (connection, incoming) =
        Connection::start(config, PreviewPorts::default(), browser).unwrap();

    assert!(matches!(
        incoming.recv_blocking().unwrap(),
        Incoming::State(ConnectionState::Connecting)
    ));
    assert!(matches!(
        incoming.recv_blocking().unwrap(),
        Incoming::State(ConnectionState::Connected)
    ));
    let Incoming::Request {
        method,
        params,
        responder,
    } = incoming.recv_blocking().unwrap()
    else {
        panic!("expected the request the relay sent");
    };
    assert_eq!(method, "app.info");
    assert_eq!(params, json!({}));
    responder.respond(Ok(json!({ "protocol": 1 })));
    connection.send_event(EVENT_WORKSPACE_CHANGED, json!({}));

    assert_eq!(seen.recv().unwrap(), "Bearer secret-key");
    assert_eq!(
        serde_json::from_str::<Value>(&seen.recv().unwrap()).unwrap(),
        json!({ "t": "res", "id": "c1:7", "ok": true, "result": { "protocol": 1 } })
    );
    assert_eq!(
        serde_json::from_str::<Value>(&seen.recv().unwrap()).unwrap(),
        json!({ "t": "evt", "event": "workspace.changed", "data": {} })
    );
}
