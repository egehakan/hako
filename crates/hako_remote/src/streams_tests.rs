use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::time::timeout;

use super::*;

#[test]
fn encodes_and_decodes_a_frame() {
    let frame = encode_stream_frame(0x0102_0304, b"hi");
    assert_eq!(frame, [0x02, 1, 2, 3, 4, b'h', b'i']);
    assert_eq!(decode_stream_frame(&frame), Some((0x0102_0304, &b"hi"[..])));
    assert_eq!(decode_stream_frame(&[0x01, 0, 0, 0, 1]), None);
    assert_eq!(decode_stream_frame(&[0x02, 0, 0]), None);
}

#[test]
fn the_send_window_closes_when_full_and_opens_with_acks() {
    let mut window = SendWindow::default();
    window.sent(STREAM_WINDOW_BYTES);
    assert_eq!(window.room(), 0);
    assert!(window.acked(1000));
    assert_eq!(window.room(), 1000);
    assert!(!window.acked(STREAM_WINDOW_BYTES as u64));
    assert_eq!(window.room(), 1000);
}

#[test]
fn the_receive_window_refuses_too_much_and_acks_in_batches() {
    let mut window = ReceiveWindow::default();
    assert!(window.received(STREAM_WINDOW_BYTES));
    assert!(!window.received(1));
    assert_eq!(window.passed_on(100, false), 0);
    assert_eq!(window.passed_on(STREAM_ACK_BYTES, false), STREAM_ACK_BYTES + 100);
    assert_eq!(window.passed_on(10, true), 10);
    assert!(window.received(STREAM_ACK_BYTES + 110));
    assert!(!window.received(1));
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Plays the relay: the table, and the queue its streams send to.
struct Relay {
    streams: Streams,
    out: mpsc::Receiver<Message>,
    ended: mpsc::UnboundedReceiver<(u32, u64)>,
}

impl Relay {
    fn new(ports: &[(&str, &[u16])]) -> Self {
        let table = PreviewPorts::default();
        table.set(
            ports
                .iter()
                .map(|(id, ports)| ((*id).to_owned(), ports.to_vec()))
                .collect::<HashMap<_, _>>(),
        );
        let (sender, out) = mpsc::channel(STREAM_QUEUE);
        let (streams, ended) = Streams::new(table, sender);
        Self {
            streams,
            out,
            ended,
        }
    }

    fn open(&mut self, sid: u32, preview: &str, port: Option<u16>) {
        self.streams.handle(Frame::StreamOpen {
            sid,
            preview: preview.to_owned(),
            port,
        });
    }

    async fn next(&mut self) -> Message {
        timeout(Duration::from_secs(5), self.out.recv())
            .await
            .expect("the stream sent nothing")
            .unwrap()
    }

    async fn next_frame(&mut self) -> Frame {
        match self.next().await {
            Message::Text(text) => Frame::decode(&text).unwrap(),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }

    /// Collects stream data until `bytes` have arrived, acknowledging as it goes when `ack`.
    async fn read_data(&mut self, sid: u32, bytes: usize, ack: bool) -> Vec<u8> {
        let mut data = Vec::new();
        while data.len() < bytes {
            match self.next().await {
                Message::Binary(frame) => {
                    let (frame_sid, payload) = decode_stream_frame(&frame).unwrap();
                    assert_eq!(frame_sid, sid);
                    data.extend_from_slice(payload);
                    if ack {
                        self.streams.handle(Frame::StreamAck {
                            sid,
                            bytes: payload.len() as u64,
                        });
                    }
                }
                Message::Text(text) => match Frame::decode(&text).unwrap() {
                    Frame::StreamAck { .. } => {}
                    other => panic!("unexpected {other:?}"),
                },
                other => panic!("unexpected {other:?}"),
            }
        }
        data
    }
}

/// A server that echoes each connection until the client closes its side, then closes its own.
async fn echo_server() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut reader, mut writer) = socket.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
                let _ = writer.shutdown().await;
            });
        }
    });
    port
}

fn closed_error(frame: &Frame) -> Option<ErrorCode> {
    match frame {
        Frame::StreamClose { error, .. } => error.as_ref().map(|error| error.code),
        _ => None,
    }
}

#[test]
fn refuses_an_unknown_preview_and_a_port_it_does_not_have() {
    runtime().block_on(async {
        let mut relay = Relay::new(&[("app", &[4000])]);
        relay.open(1, "other", None);
        assert_eq!(
            closed_error(&relay.next_frame().await),
            Some(ErrorCode::NotFound)
        );
        relay.open(2, "app", Some(4001));
        assert_eq!(
            closed_error(&relay.next_frame().await),
            Some(ErrorCode::NotFound)
        );
    });
}

#[test]
fn fails_a_port_nothing_listens_on() {
    runtime().block_on(async {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut relay = Relay::new(&[("app", &[port])]);
        relay.open(1, "app", None);
        assert_eq!(
            closed_error(&relay.next_frame().await),
            Some(ErrorCode::Failed)
        );
        timeout(Duration::from_secs(5), relay.ended.recv())
            .await
            .unwrap();
    });
}

#[test]
fn echoes_through_a_stream_and_finishes_after_both_sides_close() {
    runtime().block_on(async {
        let port = echo_server().await;
        let mut relay = Relay::new(&[("app", &[1, port])]);
        relay.open(7, "app", Some(port));
        assert_eq!(relay.next_frame().await, Frame::StreamOpened { sid: 7 });

        relay.streams.data(&encode_stream_frame(7, b"hello"));
        let mut echoed = Vec::new();
        let mut acked = 0;
        while echoed.len() < 5 || acked < 5 {
            match relay.next().await {
                Message::Binary(frame) => {
                    echoed.extend_from_slice(decode_stream_frame(&frame).unwrap().1);
                }
                Message::Text(text) => match Frame::decode(&text).unwrap() {
                    Frame::StreamAck { sid: 7, bytes } => acked += bytes,
                    other => panic!("unexpected {other:?}"),
                },
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(echoed, b"hello");

        // The relay's end of the request reaches the server, which then closes its side.
        relay.streams.handle(Frame::StreamClose {
            sid: 7,
            error: None,
        });
        assert_eq!(
            relay.next_frame().await,
            Frame::StreamClose {
                sid: 7,
                error: None
            }
        );
        let (sid, key) = timeout(Duration::from_secs(5), relay.ended.recv())
            .await
            .unwrap()
            .unwrap();
        relay.streams.ended(sid, key);
        assert!(relay.streams.is_empty());
    });
}

#[test]
fn keeps_sending_after_the_relay_has_closed_its_side() {
    runtime().block_on(async {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            socket.read_to_end(&mut request).await.unwrap();
            socket.write_all(b"response").await.unwrap();
            request
        });
        let mut relay = Relay::new(&[("app", &[port])]);
        relay.open(3, "app", None);
        assert_eq!(relay.next_frame().await, Frame::StreamOpened { sid: 3 });
        relay.streams.data(&encode_stream_frame(3, b"body"));
        relay.streams.handle(Frame::StreamClose {
            sid: 3,
            error: None,
        });
        assert_eq!(relay.read_data(3, 8, false).await, b"response");
        assert_eq!(server.await.unwrap(), b"body");
        assert_eq!(
            relay.next_frame().await,
            Frame::StreamClose {
                sid: 3,
                error: None
            }
        );
    });
}

#[test]
fn stops_at_the_window_until_the_relay_acknowledges() {
    runtime().block_on(async {
        let total = 2 * STREAM_WINDOW_BYTES;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(&vec![7; total]).await.unwrap();
            let _ = socket.read(&mut [0; 1]).await;
        });
        let mut relay = Relay::new(&[("app", &[port])]);
        relay.open(1, "app", None);
        assert_eq!(relay.next_frame().await, Frame::StreamOpened { sid: 1 });

        let first = relay.read_data(1, STREAM_WINDOW_BYTES, false).await;
        assert_eq!(first.len(), STREAM_WINDOW_BYTES);
        assert!(
            timeout(Duration::from_millis(300), relay.out.recv())
                .await
                .is_err(),
            "sent beyond the window"
        );
        relay.streams.handle(Frame::StreamAck {
            sid: 1,
            bytes: STREAM_WINDOW_BYTES as u64,
        });
        let rest = relay.read_data(1, total - first.len(), true).await;
        assert_eq!(rest.len(), total - first.len());
    });
}

#[test]
fn closes_a_stream_whose_relay_sends_beyond_the_window() {
    runtime().block_on(async {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accepts and never reads, so nothing is passed on and acknowledged.
        tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let mut relay = Relay::new(&[("app", &[port])]);
        relay.open(1, "app", None);
        assert_eq!(relay.next_frame().await, Frame::StreamOpened { sid: 1 });
        let chunk = vec![0; STREAM_CHUNK_BYTES];
        for _ in 0..(STREAM_WINDOW_BYTES / STREAM_CHUNK_BYTES) * 4 + 1 {
            relay.streams.data(&encode_stream_frame(1, &chunk));
        }
        loop {
            let frame = relay.next_frame().await;
            if let Frame::StreamAck { .. } = frame {
                continue;
            }
            assert_eq!(closed_error(&frame), Some(ErrorCode::Failed));
            break;
        }
    });
}

#[test]
fn an_error_from_the_relay_or_dropping_the_table_closes_the_connection() {
    runtime().block_on(async {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut relay = Relay::new(&[("app", &[port])]);
        relay.open(1, "app", None);
        let (mut first, _) = listener.accept().await.unwrap();
        assert_eq!(relay.next_frame().await, Frame::StreamOpened { sid: 1 });
        relay.open(2, "app", None);
        let (mut second, _) = listener.accept().await.unwrap();
        assert_eq!(relay.next_frame().await, Frame::StreamOpened { sid: 2 });

        relay.streams.handle(Frame::StreamClose {
            sid: 1,
            error: Some(RpcError::failed("gone")),
        });
        assert_eq!(relay.streams.len(), 1);
        let read = timeout(Duration::from_secs(5), first.read(&mut [0; 1])).await;
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))));

        drop(relay);
        let read = timeout(Duration::from_secs(5), second.read(&mut [0; 1])).await;
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))));
    });
}
