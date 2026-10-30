//! A scripted HTTP/1.1 upstream on real loopback TCP, shared by the kit crates' tests
//! (dev-dependency only, docs/04 §8). Requests go through the production clients unchanged,
//! so their address policy, timeouts and SSE parsing stay under test; the server only replays
//! what a test scripts and records what it received.
//!
//! Each connection carries one request and one reply. Replies are byte-exact steps, so a test
//! can split UTF-8 across writes, stall after a prefix, or hold a reply behind a [`Gate`].
//! Dropping the [`Server`] stops accepting and aborts every connection task.
#![expect(
    clippy::panic,
    reason = "test-only crate: a broken fixture fails the calling test, like the test modules it serves"
)]

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinSet;

/// Upper bound on a request the server accepts; larger requests fail the test.
const MAX_REQUEST: usize = 64 * 1024 * 1024;

/// One request as received.
#[derive(Clone)]
pub struct Request {
    /// The bytes on the wire, head and body.
    pub raw: Vec<u8>,
    /// `GET`, `POST`, …
    pub method: String,
    /// The request target, e.g. `/v1/chat/completions`.
    pub target: String,
    /// Header names lowercased, values trimmed, in arrival order.
    pub headers: Vec<(String, String)>,
    /// The body framed by `content-length` (empty without one).
    pub body: Vec<u8>,
}

impl Request {
    /// The first header named `name` (case-insensitive).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    /// The body as UTF-8, lossy.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// The whole request as UTF-8, lossy.
    #[must_use]
    pub fn raw_text(&self) -> String {
        String::from_utf8_lossy(&self.raw).into_owned()
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} ({} body bytes)",
            self.method,
            self.target,
            self.body.len()
        )
    }
}

/// A one-shot latch a reply can wait on; [`Gate::open`] releases every waiter, also later ones.
#[derive(Clone)]
pub struct Gate(Arc<watch::Sender<bool>>);

impl Gate {
    /// A closed gate.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }
    /// Opens the gate.
    pub fn open(&self) {
        self.0.send_replace(true);
    }
    /// Waits until the gate is open.
    pub async fn wait(&self) {
        let mut receiver = self.0.subscribe();
        // Only fails when the sender is gone, which `self` prevents.
        let _ = receiver.wait_for(|open| *open).await;
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

/// One step of a reply.
#[derive(Clone)]
enum Step {
    /// Write these bytes in pieces of at most `chunk` bytes, flushing after each.
    Write {
        bytes: Vec<u8>,
        chunk: usize,
    },
    Sleep(Duration),
    Wait(Gate),
    Signal(Arc<Notify>),
    /// Keep the connection open until the server is dropped.
    Stall,
}

/// What the server does on one connection after reading its request.
#[derive(Clone, Default)]
pub struct Reply(Vec<Step>);

impl Reply {
    /// Raw bytes, written at once; the connection closes afterwards.
    #[must_use]
    pub fn raw(wire: impl Into<Vec<u8>>) -> Self {
        Self::default().then_raw(wire)
    }
    /// A complete response with `content-length` and `connection: close`.
    #[must_use]
    pub fn http(status: u16, headers: &[(&str, &str)], body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!(
            "content-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        ));
        let mut wire = head.into_bytes();
        wire.extend_from_slice(&body);
        Self::raw(wire)
    }
    /// `200` with a JSON body.
    #[must_use]
    pub fn json(body: impl Into<Vec<u8>>) -> Self {
        Self::status(200, body)
    }
    /// `status` with a JSON body.
    #[must_use]
    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::http(status, &[("content-type", "application/json")], body)
    }
    /// `200 text/event-stream`, close-delimited, the body written in `chunk`-byte pieces so
    /// events and UTF-8 sequences cross reads.
    #[must_use]
    pub fn sse(body: impl Into<Vec<u8>>, chunk: usize) -> Self {
        Self::sse_head().then_chunked(body, chunk)
    }
    /// Only the head of a close-delimited `200 text/event-stream`; add the body with `then_*`.
    #[must_use]
    pub fn sse_head() -> Self {
        Self::raw("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
    }
    /// Read the request and never answer.
    #[must_use]
    pub fn silent() -> Self {
        Self(vec![Step::Stall])
    }

    /// Then write `wire` at once.
    #[must_use]
    pub fn then_raw(self, wire: impl Into<Vec<u8>>) -> Self {
        self.then_chunked(wire, usize::MAX)
    }
    /// Then write `wire` in pieces of at most `chunk` bytes.
    #[must_use]
    pub fn then_chunked(mut self, wire: impl Into<Vec<u8>>, chunk: usize) -> Self {
        self.0.push(Step::Write {
            bytes: wire.into(),
            chunk: chunk.max(1),
        });
        self
    }
    /// Then pause.
    #[must_use]
    pub fn then_sleep(mut self, duration: Duration) -> Self {
        self.0.push(Step::Sleep(duration));
        self
    }
    /// Then wait for `gate`.
    #[must_use]
    pub fn then_wait(mut self, gate: &Gate) -> Self {
        self.0.push(Step::Wait(gate.clone()));
        self
    }
    /// Then notify `notify` (`notify_one`, so a later waiter still sees it).
    #[must_use]
    pub fn then_signal(mut self, notify: &Arc<Notify>) -> Self {
        self.0.push(Step::Signal(Arc::clone(notify)));
        self
    }
    /// Then keep the connection open until the server is dropped.
    #[must_use]
    pub fn then_stall(mut self) -> Self {
        self.0.push(Step::Stall);
        self
    }
    /// Read the request, then hang up without a byte (EOF, not a reset).
    #[must_use]
    pub fn close() -> Self {
        Self::default()
    }
}

/// What the n-th connection gets once the script is used up.
#[derive(Clone)]
pub enum Exhausted {
    /// The last scripted reply again.
    RepeatLast,
    /// This reply.
    Reply(Reply),
}

type Handler = Arc<dyn Fn(&Request) -> Reply + Send + Sync>;

/// A loopback upstream; dropping it stops it.
pub struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Request>>>,
    requests: mpsc::UnboundedReceiver<Request>,
    tasks: JoinSet<()>,
}

impl Server {
    /// The n-th connection gets `replies[n]`, later ones per `exhausted`.
    ///
    /// # Panics
    /// When `replies` is empty and `exhausted` is [`Exhausted::RepeatLast`].
    pub async fn script(replies: Vec<Reply>, exhausted: Exhausted) -> Self {
        assert!(
            !replies.is_empty() || matches!(exhausted, Exhausted::Reply(_)),
            "an empty script needs an explicit Exhausted::Reply"
        );
        let next = Mutex::new(0_usize);
        Self::handler(move |_| {
            let mut index = next.lock().unwrap_or_else(PoisonError::into_inner);
            let reply = match (replies.get(*index), &exhausted) {
                (Some(reply), _) => reply.clone(),
                (None, Exhausted::Reply(reply)) => reply.clone(),
                (None, Exhausted::RepeatLast) => replies[replies.len() - 1].clone(),
            };
            *index += 1;
            reply
        })
        .await
    }

    /// Every connection gets `handler(request)`; requests are read concurrently.
    ///
    /// # Panics
    /// When no loopback port can be bound.
    pub async fn handler(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("bind loopback: {error}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("loopback address: {error}"))
            .port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (sender, requests) = mpsc::unbounded_channel();
        let handler: Handler = Arc::new(handler);
        let mut tasks = JoinSet::new();
        let log = Arc::clone(&seen);
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((socket, _)) = listener.accept().await {
                let (handler, log, sender) =
                    (Arc::clone(&handler), Arc::clone(&log), sender.clone());
                connections.spawn(serve(socket, handler, log, sender));
            }
        });
        Self {
            port,
            seen,
            requests,
            tasks,
        }
    }

    /// `http://127.0.0.1:<port>`, without a trailing slash.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    /// The bound port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
    /// Every request received so far, in arrival order.
    #[must_use]
    pub fn seen(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// Waits for the next request; requests already received are returned first.
    ///
    /// # Panics
    /// After 10 seconds without a request.
    pub async fn request(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(10), self.requests.recv())
            .await
            .unwrap_or_else(|_| panic!("no request within 10s"))
            .unwrap_or_else(|| panic!("server stopped"))
    }
    /// Asserts that no request arrives within `within`.
    ///
    /// # Panics
    /// When one does.
    pub async fn no_request(&mut self, within: Duration) {
        if let Ok(Some(request)) = tokio::time::timeout(within, self.requests.recv()).await {
            panic!("unexpected request {request:?}");
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}

async fn serve(
    mut socket: TcpStream,
    handler: Handler,
    log: Arc<Mutex<Vec<Request>>>,
    sender: mpsc::UnboundedSender<Request>,
) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let reply = handler(&request);
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request.clone());
    let _ = sender.send(request);
    for step in reply.0 {
        match step {
            Step::Write { bytes, chunk } => {
                for piece in bytes.chunks(chunk) {
                    if socket.write_all(piece).await.is_err() || socket.flush().await.is_err() {
                        return;
                    }
                }
            }
            Step::Sleep(duration) => tokio::time::sleep(duration).await,
            Step::Wait(gate) => gate.wait().await,
            Step::Signal(notify) => notify.notify_one(),
            Step::Stall => std::future::pending::<()>().await,
        }
    }
    let _ = socket.shutdown().await;
}

/// Reads one request framed by `content-length`; `None` when the client hung up first.
async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 8192];
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&buffer[..read]);
        assert!(
            raw.len() <= MAX_REQUEST,
            "request head over {MAX_REQUEST} bytes"
        );
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let method = start.next().unwrap_or_default().to_owned();
    let target = start.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        length <= MAX_REQUEST,
        "request body over {MAX_REQUEST} bytes"
    );
    let body_start = head_end + 4;
    while raw.len() < body_start + length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    let body = raw[body_start..raw.len().min(body_start + length)].to_vec();
    Some(Request {
        raw,
        method,
        target,
        headers,
        body,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
