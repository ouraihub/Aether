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

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_support::net::AddressPolicy;
use aether_support::{Connection, KitErrorCode, Protocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    Approval, Approver, BoxFuture, Budget, EndStatus, Message, RunEvent, RunInput, RunSummary,
    Runner, SystemPart, ToolCall, ToolExecution, ToolExecutor, Usage,
};

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
    fn authorize(&self, _call: &ToolCall, _default: Approval) -> Approval {
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
        api_key: Some(Zeroizing::new(KEY.to_owned())),
        model_id: Some("model-x".to_owned()),
        endpoint_id: None,
        context_window: None,
        max_output: None,
        supports_tools: None,
        reasoning: None,
        native_web_search: false,
    };
    RunInput {
        run_id: "r1".to_owned(),
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
        tools: Vec::new(),
        reasoning_effort: aether_support::ReasoningEffort::Default,
        native_web_search: false,
    }
}

#[tokio::test]
async fn native_search_keeps_citations_and_does_not_execute_server_calls_locally() {
    for protocol in [Protocol::AnthropicMessages, Protocol::OpenaiResponses] {
        let body = if protocol == Protocol::AnthropicMessages {
            concat!("data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srv\",\"name\":\"web_search\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Answer\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"citations_delta\",\"citation\":{\"url\":\"https://example.com/source\"}}}\n\n",
                    "data: {\"type\":\"message_stop\"}\n\n")
        } else {
            concat!("data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"web_search_call\",\"id\":\"srv\"}}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Answer\"}\n\n",
                    "data: {\"type\":\"response.output_text.annotation.added\",\"annotation\":{\"type\":\"url_citation\",\"url\":\"https://example.com/source\"}}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n")
        }.to_owned();
        let (base, seen) = upstream(Reply::Sse {
            cut: body.len(),
            body,
        })
        .await;
        let mut req = input(protocol, base);
        req.connection.native_web_search = true;
        req.native_web_search = true;
        let (summary, events) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        assert_eq!(
            streamed(&events),
            "Answer [source](<https://example.com/source>)"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, RunEvent::ToolCall { .. }))
        );
        let requests = seen.lock().await;
        let encoded: serde_json::Value =
            serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            encoded["tools"][0]["type"],
            if protocol == Protocol::AnthropicMessages {
                "web_search_20250305"
            } else {
                "web_search"
            }
        );
    }
}

fn runner(first_byte_ms: u64, idle_ms: u64) -> Runner {
    let mut runner = Runner::new(AddressPolicy::default()).unwrap();
    runner.first_byte = Duration::from_millis(first_byte_ms);
    runner.idle = Duration::from_millis(idle_ms);
    runner
}

/// Runs once; `cancel_on_text` cancels when the first delta arrives.
async fn run(
    runner: &Runner,
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
async fn streams_text_and_usage_for_every_protocol() {
    for protocol in PROTOCOLS {
        for name in ["text", "chinese", "truncated"] {
            let want = expected(protocol, name);
            let text = want["text"].as_str().unwrap();
            let i = u32::try_from(want["inputTokens"].as_u64().unwrap()).unwrap();
            let o = u32::try_from(want["outputTokens"].as_u64().unwrap()).unwrap();
            let body = fixture(protocol, name);
            let cut = body.len();
            let (base, seen) = upstream(Reply::Sse { body, cut }).await;
            let input = input(protocol, base);
            let (summary, events) = run(&runner(5_000, 5_000), input, false).await;
            assert_eq!(
                end(&events),
                (EndStatus::Completed, None, String::new()),
                "{protocol:?} {name}"
            );
            assert!(matches!(events[0], RunEvent::RoundStart { round: 1 }));
            assert_eq!(streamed(&events), text, "{protocol:?} {name}");
            assert_eq!(summary_text(&summary), text);
            let expected = Usage {
                input_tokens: Some(i),
                output_tokens: Some(o),
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            };
            assert_eq!(summary.usage, expected, "{protocol:?} {name}");
            assert!(matches!(
                events[events.len() - 2],
                RunEvent::Usage { input_tokens, output_tokens, .. } if input_tokens == Some(i) && output_tokens == Some(o)
            ));

            let request = seen.lock().await[0].clone();
            let (path, system, auth) = match protocol {
                Protocol::AnthropicMessages => {
                    ("/v1/messages", r#""system":"be brief""#, "x-api-key: ")
                }
                Protocol::OpenaiChat => (
                    "/v1/chat/completions",
                    r#"{"content":"be brief","role":"system"}"#,
                    "authorization: Bearer ",
                ),
                Protocol::OpenaiResponses => (
                    "/v1/responses",
                    r#""instructions":"be brief""#,
                    "authorization: Bearer ",
                ),
            };
            let lower = request.to_ascii_lowercase();
            assert!(
                request.starts_with(&format!("POST {path} HTTP/1.1")),
                "{request}"
            );
            assert!(
                lower.contains(&format!("{}{KEY}", auth.to_ascii_lowercase()).to_ascii_lowercase()),
                "{request}"
            );
            assert!(
                request.contains(system) && request.contains(r#""stream":true"#),
                "{request}"
            );
            if protocol == Protocol::AnthropicMessages {
                assert!(request.contains(r#"{"content":[{"text":"reply","type":"text"}],"role":"assistant"},{"content":"hi","role":"user"}"#), "{request}");
            } else {
                assert!(request.contains(r#"{"content":"earlier","role":"user"},{"content":"reply","role":"assistant"},{"content":"hi","role":"user"}"#), "{request}");
            }
            if protocol == Protocol::AnthropicMessages {
                assert!(lower.contains("anthropic-version: 2023-06-01"));
            }
            if protocol == Protocol::OpenaiChat {
                assert!(request.contains(r#""stream_options":{"include_usage":true}"#));
            }
        }
    }
}

#[tokio::test]
async fn in_stream_upstream_error_fails_and_keeps_partial_text() {
    for protocol in PROTOCOLS {
        let want = expected(protocol, "upstream-error");
        let partial = want["text"].as_str().unwrap();
        let message = want["errorMessage"].as_str().unwrap();
        assert_eq!(want["errorCode"], "upstream");
        let body = fixture(protocol, "upstream-error");
        let cut = body.len();
        let (base, _) = upstream(Reply::Sse { body, cut }).await;
        let (summary, events) = run(&runner(5_000, 5_000), input(protocol, base), false).await;
        let (status, code, text) = end(&events);
        assert_eq!(
            (status, code),
            (EndStatus::Failed, Some(KitErrorCode::Upstream)),
            "{protocol:?}"
        );
        assert!(text.contains(message), "{text}");
        assert_eq!(summary.status, EndStatus::Failed);
        assert_eq!(summary_text(&summary), partial);
        assert!(!events.iter().any(|e| matches!(e, RunEvent::Usage { .. })));
    }
}

#[tokio::test]
async fn non_2xx_is_classified_and_sanitized() {
    let leaky = format!(r#"{{"error":{{"message":"bad key {KEY}\nline two"}}}}"#);
    for (status, code) in [
        (401, KitErrorCode::Unauthorized),
        (429, KitErrorCode::RateLimited),
        (500, KitErrorCode::Upstream),
    ] {
        for protocol in PROTOCOLS {
            let (base, _) = upstream(Reply::Status(status, leaky.clone())).await;
            let (summary, events) = run(&runner(5_000, 5_000), input(protocol, base), false).await;
            let (end_status, end_code, message) = end(&events);
            assert_eq!((end_status, end_code), (EndStatus::Failed, Some(code)));
            assert!(
                message.starts_with(&format!("{status}: ")) && message.contains("bad key"),
                "{message}"
            );
            assert!(
                !message.contains(KEY) && !message.contains('\n'),
                "{message}"
            );
            assert!(summary.messages.is_empty());
        }
    }
}

#[tokio::test]
async fn first_byte_timeout_fires_before_and_after_headers() {
    for protocol in PROTOCOLS {
        for reply in [
            Reply::Silent,
            Reply::Sse {
                body: fixture(protocol, "text"),
                cut: 0,
            },
        ] {
            let (base, _) = upstream(reply).await;
            let started = Instant::now();
            let (_, events) = run(&runner(300, 10_000), input(protocol, base), false).await;
            let (status, code, _) = end(&events);
            assert_eq!(
                (status, code),
                (EndStatus::Failed, Some(KitErrorCode::Timeout))
            );
            assert!(started.elapsed() < Duration::from_secs(5));
            let RunEvent::End {
                error: Some(error), ..
            } = events.last().unwrap()
            else {
                unreachable!()
            };
            assert_eq!(error.details().reason(), Some("first_byte"));
        }
    }
}

/// Bytes up to the end of the event carrying the first text delta.
fn after_first_delta(body: &str) -> usize {
    let at = [
        "\"text_delta\"",
        "\"content\":\"Hello\"",
        "output_text.delta",
    ]
    .iter()
    .filter_map(|needle| body.find(needle))
    .min()
    .unwrap();
    at + body[at..].find("\n\n").unwrap() + 2
}

#[tokio::test]
async fn idle_timeout_fires_after_output_started() {
    for protocol in PROTOCOLS {
        let body = fixture(protocol, "text");
        let cut = after_first_delta(&body);
        let (base, _) = upstream(Reply::Sse { body, cut }).await;
        let (summary, events) = run(&runner(10_000, 300), input(protocol, base), false).await;
        let (status, code, _) = end(&events);
        assert_eq!(
            (status, code),
            (EndStatus::Failed, Some(KitErrorCode::Timeout)),
            "{protocol:?}"
        );
        let RunEvent::End {
            error: Some(error), ..
        } = events.last().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(error.details().reason(), Some("idle"));
        assert_eq!(summary_text(&summary), "Hello");
    }
}

#[tokio::test]
async fn cancel_mid_stream_ends_cancelled_at_once() {
    for protocol in PROTOCOLS {
        let body = fixture(protocol, "text");
        let cut = after_first_delta(&body);
        let (base, _) = upstream(Reply::Sse { body, cut }).await;
        let started = Instant::now();
        let (summary, events) = run(&runner(10_000, 10_000), input(protocol, base), true).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(end(&events), (EndStatus::Cancelled, None, String::new()));
        assert_eq!(summary.status, EndStatus::Cancelled);
        assert_eq!(summary_text(&summary), "Hello");
    }
}

#[tokio::test]
async fn dropped_run_still_emits_one_end() {
    let (base, _) = upstream(Reply::Silent).await;
    let input = input(Protocol::OpenaiChat, base);
    let runner = runner(10_000, 10_000);
    let cancel = CancellationToken::new();
    let mut events = Vec::new();
    let run = runner.run(input, &NoTools, &NoTools, &cancel, |event| {
        events.push(event)
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), run)
            .await
            .is_err()
    );
    assert_eq!(end(&events), (EndStatus::Cancelled, None, String::new()));
}

#[tokio::test]
async fn stream_without_terminal_event_fails() {
    let body = fixture(Protocol::AnthropicMessages, "text");
    let cut = after_first_delta(&body);
    let (base, _) = upstream(Reply::Sse {
        body: body[..cut].to_owned(),
        cut,
    })
    .await;
    let (summary, events) = run(
        &runner(5_000, 5_000),
        input(Protocol::AnthropicMessages, base),
        false,
    )
    .await;
    assert_eq!(end(&events).1, Some(KitErrorCode::Network));
    assert_eq!(summary_text(&summary), "Hello");
}
