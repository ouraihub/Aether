//! Protocol layer against a loopback upstream replaying `packages/ui/fixtures/sse` (docs/12 §3).
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable,
    clippy::wildcard_enum_match_arm,
    reason = "a failed assertion is a failed test"
)]

mod tool_loop;
mod tools;
mod validate;
mod protocol;
mod stream;

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_support::net::AddressPolicy;
use aether_support::{
    Connection, KitErrorCode, Protocol, ReasoningConfig, ReasoningDialect, ReasoningEffort,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    Approval, Approver, AuthorizeContext, BoxFuture, Budget, EndStatus, Message, PermissionMode,
    RunEngine, RunEvent, RunInput, RunSummary, SystemPart, ToolCall, ToolExecution, ToolExecutor,
    Usage,
};
use aether_images::ImagePart;

const KEY: &str = "sk-test-0123456789abcdefghijkl";
const PROTOCOLS: [Protocol; 3] = [
    Protocol::AnthropicMessages,
    Protocol::OpenaiChat,
    Protocol::OpenaiResponses,
];

/// How the fake upstream answers.
#[derive(Clone)]
enum Reply {
    /// 200 SSE: `body[..cut]`, then stall forever if `cut < body.len()`, else close.
    Sse { body: String, cut: usize },
    /// Accept the request and never answer.
    Silent,
    /// A non-2xx JSON error.
    Status(u16, String),
}

/// Raw requests the fake upstream received, in order.
type Seen = Arc<Mutex<Vec<String>>>;

/// Returns the base URL and the raw requests seen.
async fn upstream(reply: Reply) -> (String, Seen) {
    script(vec![reply]).await
}

/// The n-th request gets `replies[n]`; later ones get the last reply.
async fn script(replies: Vec<Reply>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut next = 0;
        while let Ok((mut socket, _)) = listener.accept().await {
            let raw = read_request(&mut socket).await;
            log.lock().await.push(raw);
            let reply = replies[next.min(replies.len() - 1)].clone();
            next += 1;
            tokio::spawn(async move {
                match reply {
                    Reply::Sse { body, cut } => {
                        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
                        let _ = socket.write_all(head.as_bytes()).await;
                        // Small pieces so deltas cross chunk (and UTF-8) boundaries.
                        for piece in body.as_bytes()[..cut].chunks(7) {
                            let _ = socket.write_all(piece).await;
                            let _ = socket.flush().await;
                        }
                        if cut < body.len() {
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                        }
                    }
                    Reply::Silent => tokio::time::sleep(Duration::from_secs(3600)).await,
                    Reply::Status(status, body) => {
                        let response = format!(
                            "HTTP/1.1 {status} Err\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), seen)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        let n = socket.read(&mut buf).await.unwrap();
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if raw.len() >= end + 4 + length || n == 0 {
                return String::from_utf8_lossy(&raw).into_owned();
            }
        }
        if n == 0 {
            return String::from_utf8_lossy(&raw).into_owned();
        }
    }
}

fn dir(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::AnthropicMessages => "anthropic-messages",
        Protocol::OpenaiChat => "openai-chat",
        Protocol::OpenaiResponses => "openai-responses",
    }
}

fn fixture(protocol: Protocol, name: &str) -> String {
    let path = format!(
        "{}/../../packages/ui/fixtures/sse/{}/{name}.sse",
        env!("CARGO_MANIFEST_DIR"),
        dir(protocol)
    );
    std::fs::read_to_string(path).unwrap()
}

/// The outcome of replaying one fixture, shared with the TypeScript side (`expect.json`).
fn expected(protocol: Protocol, name: &str) -> serde_json::Value {
    let text = include_str!("../../../packages/ui/fixtures/sse/expect.json");
    let all: serde_json::Value = serde_json::from_str(text).unwrap();
    all[dir(protocol)][name].clone()
}

/// A complete 200 SSE reply.
fn sse(body: String) -> Reply {
    Reply::Sse {
        cut: body.len(),
        body,
    }
}

/// For runs without tools: any call is denied, nothing executes or waits.
struct NoTools;

#[async_trait::async_trait]
impl ToolExecutor for NoTools {
    fn authorize(&self, _call: &ToolCall, _ctx: &AuthorizeContext<'_>) -> Approval {
        Approval::Deny
    }
    async fn execute(&self, _call: &ToolCall, _cancel: &CancellationToken) -> ToolExecution {
        unreachable!("denied calls never execute")
    }
}

impl Approver for NoTools {
    fn wait(
        &self,
        _run: &str,
        _call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> BoxFuture<'_, bool> {
        unreachable!("denied calls never wait")
    }
}

fn input(protocol: Protocol, base_url: String) -> RunInput {
    let connection = Connection {
        base_url,
        protocol,
        api_keys: vec![Zeroizing::new(KEY.to_owned())],
        model_id: Some("model-x".to_owned()),
        endpoint_id: None,
        context_window: None,
        max_output: None,
        supports_tools: None,
        reasoning: None,
        native_web_search: false,
        can_vision: true,
        pricing: None,
        headers: Vec::new(),
        image_base_url: None,
        image_edit_base_url: None,
    };
    RunInput {
        run_id: "r1".to_owned(),
        purpose: "chat".to_owned(),
        permission_mode: PermissionMode::Default,
        budget: Budget::default().for_connection(&connection),
        connection,
        system: vec![SystemPart::Host("be brief".to_owned())],
        history: vec![
            Message::User("earlier".to_owned()),
            Message::Assistant {
                text: "reply".to_owned(),
                tool_calls: Vec::new(),
                reasoning: Vec::new(),
            },
        ],
        input: "hi".to_owned(),
        images: Vec::new(),
        tools: Vec::new(),
        reasoning_effort: aether_support::ReasoningEffort::Default,
        native_web_search: false,
    }
}


fn runner(first_byte_ms: u64, idle_ms: u64) -> RunEngine {
    let mut runner = RunEngine::new(AddressPolicy::default()).unwrap();
    runner.first_byte = Duration::from_millis(first_byte_ms);
    runner.idle = Duration::from_millis(idle_ms);
    runner
}

/// Runs once; `cancel_on_text` cancels when the first delta arrives.
async fn run(
    runner: &RunEngine,
    input: RunInput,
    cancel_on_text: bool,
) -> (RunSummary, Vec<RunEvent>) {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let mut events = Vec::new();
    let summary = runner
        .run(input, &NoTools, &NoTools, &cancel, |event| {
            if cancel_on_text && matches!(event, RunEvent::TextDelta { .. }) {
                trigger.cancel();
            }
            events.push(event);
        })
        .await;
    (summary, events)
}

/// Exactly one `end`, last; returns its status and error code.
fn end(events: &[RunEvent]) -> (EndStatus, Option<KitErrorCode>, String) {
    let ends = events
        .iter()
        .filter(|e| matches!(e, RunEvent::End { .. }))
        .count();
    assert_eq!(ends, 1, "{events:?}");
    match events.last().unwrap() {
        RunEvent::End { status, error, .. } => (
            *status,
            error.as_ref().map(aether_support::KitError::code),
            error
                .as_ref()
                .map(|e| e.message().to_owned())
                .unwrap_or_default(),
        ),
        other => panic!("last event is {other:?}"),
    }
}

fn streamed(events: &[RunEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn summary_text(summary: &RunSummary) -> &str {
    match summary.messages.as_slice() {
        [Message::Assistant { text, .. }] => text,
        [] => "",
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
