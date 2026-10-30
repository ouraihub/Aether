#![expect(clippy::unwrap_used, reason = "failed assertions fail tests")]

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;

use super::{Exhausted, Gate, Reply, Server};

/// Sends `request` raw and returns everything the server wrote before closing.
async fn exchange(server: &Server, request: &[u8]) -> Vec<u8> {
    let mut socket = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    socket.write_all(request).await.unwrap();
    let mut answer = Vec::new();
    socket.read_to_end(&mut answer).await.unwrap();
    answer
}

const POST: &[u8] = b"POST /v1/x HTTP/1.1\r\nHost: a\r\nContent-Type: application/json\r\ncontent-length: 7\r\n\r\n{\"a\":1}";

#[tokio::test]
async fn records_the_framed_request_and_follows_the_script() {
    let mut server = Server::script(
        vec![Reply::status(503, "{}"), Reply::json("{\"ok\":true}")],
        Exhausted::Reply(Reply::status(500, "late")),
    )
    .await;
    let first = String::from_utf8(exchange(&server, POST).await).unwrap();
    assert!(
        first.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "{first}"
    );
    let second = String::from_utf8(exchange(&server, POST).await).unwrap();
    assert!(second.ends_with("\r\n\r\n{\"ok\":true}"), "{second}");
    assert!(second.contains("content-length: 11\r\n"), "{second}");
    let third = String::from_utf8(exchange(&server, POST).await).unwrap();
    assert!(third.starts_with("HTTP/1.1 500 "), "{third}");

    let request = server.request().await;
    assert_eq!(
        (request.method.as_str(), request.target.as_str()),
        ("POST", "/v1/x")
    );
    assert_eq!(request.header("CONTENT-TYPE"), Some("application/json"));
    assert_eq!(request.text(), "{\"a\":1}");
    assert_eq!(request.raw, POST);
    assert_eq!(server.seen().len(), 3);
}

#[tokio::test]
async fn repeat_last_and_binary_bodies_stay_byte_exact() {
    let server = Server::script(
        vec![Reply::http(200, &[], vec![0x89, b'P', b'N', b'G'])],
        Exhausted::RepeatLast,
    )
    .await;
    for _ in 0..2 {
        let answer = exchange(&server, b"GET / HTTP/1.1\r\n\r\n").await;
        assert!(answer.ends_with(&[b'\n', 0x89, b'P', b'N', b'G']));
    }
}

#[tokio::test]
async fn sse_pieces_split_utf8_and_a_gate_holds_the_rest() {
    let gate = Gate::new();
    let sent = Arc::new(Notify::new());
    let server = Server::script(
        vec![
            Reply::sse_head()
                .then_chunked("data: 你\n\n", 1)
                .then_signal(&sent)
                .then_wait(&gate)
                .then_raw("data: end\n\n"),
        ],
        Exhausted::RepeatLast,
    )
    .await;
    let mut socket = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    socket.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    sent.notified().await;
    let mut buffer = vec![0_u8; 4096];
    let mut prefix = Vec::new();
    while !prefix.ends_with("你\n\n".as_bytes()) {
        let read = socket.read(&mut buffer).await.unwrap();
        prefix.extend_from_slice(&buffer[..read]);
    }
    // Held: nothing more arrives until the gate opens.
    let held = tokio::time::timeout(Duration::from_millis(50), socket.read(&mut buffer)).await;
    assert!(held.is_err(), "the gated rest arrived early");
    gate.open();
    let mut rest = Vec::new();
    socket.read_to_end(&mut rest).await.unwrap();
    assert_eq!(rest, b"data: end\n\n");
}

#[tokio::test]
async fn close_hangs_up_and_handler_sees_each_request() {
    let server = Server::handler(|request| {
        if request.target == "/gone" {
            Reply::close()
        } else {
            Reply::json(request.target.clone())
        }
    })
    .await;
    assert!(
        exchange(&server, b"GET /gone HTTP/1.1\r\n\r\n")
            .await
            .is_empty()
    );
    let answer = exchange(&server, b"GET /echo HTTP/1.1\r\n\r\n").await;
    assert!(answer.ends_with(b"/echo"));
}

#[tokio::test]
async fn no_request_passes_quietly_and_drop_releases_stalled_connections() {
    let mut server = Server::script(vec![Reply::silent()], Exhausted::RepeatLast).await;
    server.no_request(Duration::from_millis(20)).await;
    let mut socket = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    socket.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let _ = server.request().await;
    drop(server);
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut rest)).await;
    assert!(
        matches!(read, Ok(Ok(0)) | Ok(Err(_))),
        "stalled connection survived the server"
    );
}
