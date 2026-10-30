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
mod traces;
mod validate;

use std::time::{Duration, Instant};

use aether_support::net::AddressPolicy;
use aether_support::{
    Connection, KitErrorCode, Protocol, ReasoningConfig, ReasoningDialect, ReasoningEffort,
};
use aether_testkit::{Exhausted, Reply, Server};
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

/// Bytes per write, so SSE events and UTF-8 sequences cross reads.
const CHUNK: usize = 7;

/// Returns the base URL and the upstream; every request gets `reply`.
async fn upstream(reply: Reply) -> (String, Server) {
    script(vec![reply]).await
}

/// The n-th request gets `replies[n]`; later ones get the last reply. The upstream stops when
/// the returned [`Server`] is dropped.
async fn script(replies: Vec<Reply>) -> (String, Server) {
    let server = Server::script(replies, Exhausted::RepeatLast).await;
    (format!("{}/v1", server.url()), server)
}

/// The raw requests the upstream received so far, in order.
fn requests(server: &Server) -> Vec<String> {
    server
        .seen()
        .iter()
        .map(aether_testkit::Request::raw_text)
        .collect()
}

/// 200 SSE: `body[..cut]`, then stall until the upstream is dropped if `cut < body.len()`,
/// else close.
fn sse_cut(body: &str, cut: usize) -> Reply {
    let reply = Reply::sse(&body.as_bytes()[..cut], CHUNK);
    if cut < body.len() {
        reply.then_stall()
    } else {
        reply
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
    Reply::sse(body, CHUNK)
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
        let (base, server) = upstream(sse(body)).await;
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
        let sent = requests(&server);
        let encoded: serde_json::Value =
            serde_json::from_str(sent[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
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

#[tokio::test]
async fn usage_counts_cache_and_reasoning_and_prices_the_turn() {
    let pricing = aether_support::ModelPricing {
        currency: aether_support::Currency::Usd,
        tiers: vec![aether_support::PricingTier {
            min_input_tokens: 0,
            input: 10.0,
            output: 20.0,
            cache_read: Some(1.0),
            cache_write: None,
        }],
    };
    for protocol in PROTOCOLS {
        let body = match protocol {
            Protocol::AnthropicMessages => concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":30,\"cache_read_input_tokens\":80,\"cache_creation_input_tokens\":10,\"output_tokens\":1}}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":30}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ),
            Protocol::OpenaiChat => concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":30,\"prompt_tokens_details\":{\"cached_tokens\":80,\"cache_write_tokens\":10},\"completion_tokens_details\":{\"reasoning_tokens\":20}}}\n\n",
                "data: [DONE]\n\n",
            ),
            Protocol::OpenaiResponses => concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":120,\"output_tokens\":30,\"input_tokens_details\":{\"cached_tokens\":80,\"cache_write_tokens\":10},\"output_tokens_details\":{\"reasoning_tokens\":20}}}}\n\n",
            ),
        };
        let (base, _server) = upstream(sse(body.to_owned())).await;
        let mut req = input(protocol, base);
        req.connection.pricing = Some(pricing.clone());
        let (summary, events) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(summary.status, EndStatus::Completed, "{protocol:?}");
        let reasoning = (protocol != Protocol::AnthropicMessages).then_some(20);
        assert_eq!(
            summary.usage,
            Usage {
                input_tokens: Some(120),
                output_tokens: Some(30),
                cache_read_tokens: Some(80),
                cache_write_tokens: Some(10),
                reasoning_tokens: reasoning,
            },
            "{protocol:?}"
        );
        let context = summary.context_usage.as_ref().unwrap();
        assert_eq!(context.tokens, Some(150));
        assert_eq!(context.source, aether_support::ContextSource::Reported);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, RunEvent::Context { usage } if usage == context))
        );
        // (30·10 + 80·1 + 10·10 + 30·20) / 1e6
        let cost = summary.cost.unwrap();
        assert!(
            (cost.amount - 0.001_08).abs() < 1e-12,
            "{protocol:?} {cost:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            RunEvent::Usage { cost: Some(c), .. } if *c == cost
        )));
    }
}

fn request_body(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[tokio::test]
async fn custom_headers_are_sent_and_never_replace_auth() {
    for protocol in PROTOCOLS {
        let (base, server) = upstream(sse(fixture(protocol, "text"))).await;
        let mut req = input(protocol, base);
        req.connection.headers = vec![
            ("X-Relay-Tag".to_owned(), "team-a".to_owned()),
            // Rejected at save; skipped here too so the key always wins.
            ("Authorization".to_owned(), "Bearer forged".to_owned()),
        ];
        let (summary, _) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let raw = requests(&server)[0].to_ascii_lowercase();
        assert!(raw.contains("x-relay-tag: team-a"), "{raw}");
        assert!(raw.contains(&format!(
            "authorization: bearer {}",
            KEY.to_ascii_lowercase()
        )));
        assert!(!raw.contains("forged"), "{raw}");
    }
}

#[tokio::test]
async fn reasoning_default_and_none_are_encoded_per_protocol() {
    for protocol in PROTOCOLS {
        let (base, server) = upstream(sse(fixture(protocol, "text"))).await;
        let (summary, _) = run(&runner(5_000, 5_000), input(protocol, base), false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let body = request_body(&requests(&server)[0]);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("thinking").is_none());

        let (base, server) = upstream(sse(fixture(protocol, "text"))).await;
        let mut high = input(protocol, base);
        high.reasoning_effort = ReasoningEffort::High;
        high.connection.reasoning = Some(ReasoningConfig {
            dialect: if protocol == Protocol::AnthropicMessages {
                ReasoningDialect::AnthropicBudget
            } else {
                ReasoningDialect::Openai
            },
            efforts: vec![ReasoningEffort::High],
            budget_tokens: (protocol == Protocol::AnthropicMessages).then_some(3_000),
        });
        let (summary, _) = run(&runner(5_000, 5_000), high, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let encoded = request_body(&requests(&server)[0]);
        match protocol {
            Protocol::AnthropicMessages => assert_eq!(
                encoded["thinking"],
                serde_json::json!({"type":"enabled","budget_tokens":2400})
            ),
            Protocol::OpenaiChat => assert_eq!(encoded["reasoning_effort"], "high"),
            Protocol::OpenaiResponses => assert_eq!(encoded["reasoning"]["effort"], "high"),
        }

        let (base, server) = upstream(sse(fixture(protocol, "text"))).await;
        let mut req = input(protocol, base);
        req.reasoning_effort = ReasoningEffort::None;
        req.connection.reasoning = Some(ReasoningConfig {
            dialect: if protocol == Protocol::AnthropicMessages {
                ReasoningDialect::AnthropicBudget
            } else {
                ReasoningDialect::Openai
            },
            efforts: vec![ReasoningEffort::None, ReasoningEffort::High],
            budget_tokens: (protocol == Protocol::AnthropicMessages).then_some(3_000),
        });
        let (summary, _) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let body = request_body(&requests(&server)[0]);
        match protocol {
            Protocol::AnthropicMessages => {
                assert_eq!(body["thinking"], serde_json::json!({"type":"disabled"}))
            }
            Protocol::OpenaiChat => assert_eq!(body["reasoning_effort"], "none"),
            // A non-OpenAI Responses server gets no `summary` (third-party servers may 400).
            Protocol::OpenaiResponses => {
                assert_eq!(body["reasoning"], serde_json::json!({"effort":"none"}))
            }
        }
    }
}

#[tokio::test]
async fn reasoning_budget_dialects_clamp_before_encoding() {
    for dialect in [
        ReasoningDialect::AnthropicBudget,
        ReasoningDialect::AnthropicAdaptive,
    ] {
        let (base, server) = upstream(sse(fixture(Protocol::AnthropicMessages, "text"))).await;
        let mut req = input(Protocol::AnthropicMessages, base);
        req.budget.max_output = 2_000;
        req.reasoning_effort = ReasoningEffort::High;
        req.connection.reasoning = Some(ReasoningConfig {
            dialect,
            efforts: vec![ReasoningEffort::High],
            budget_tokens: (dialect == ReasoningDialect::AnthropicBudget).then_some(3_000),
        });
        let (summary, _) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let body = request_body(&requests(&server)[0]);
        if dialect == ReasoningDialect::AnthropicBudget {
            assert_eq!(
                body["thinking"],
                serde_json::json!({"type":"enabled","budget_tokens":1_999})
            );
            assert!(body.get("output_config").is_none());
        } else {
            assert_eq!(body["thinking"], serde_json::json!({"type":"adaptive"}));
            assert_eq!(body["output_config"]["effort"], "high");
        }
    }
}

#[tokio::test]
async fn vendor_reasoning_dialects_write_their_own_fields() {
    use serde_json::json;
    let cases = [
        (
            ReasoningDialect::ThinkingType,
            None,
            ReasoningEffort::None,
            json!({"thinking":{"type":"disabled"}}),
        ),
        (
            ReasoningDialect::ThinkingType,
            None,
            ReasoningEffort::Auto,
            json!({"thinking":{"type":"enabled"}}),
        ),
        (
            ReasoningDialect::ThinkingType,
            None,
            ReasoningEffort::High,
            json!({"thinking":{"type":"enabled"},"reasoning_effort":"high"}),
        ),
        (
            ReasoningDialect::Qwen,
            Some(10_000),
            ReasoningEffort::None,
            json!({"enable_thinking":false}),
        ),
        (
            ReasoningDialect::Qwen,
            Some(10_000),
            ReasoningEffort::Auto,
            json!({"enable_thinking":true}),
        ),
        (
            ReasoningDialect::Qwen,
            Some(10_000),
            ReasoningEffort::Low,
            json!({"enable_thinking":true,"thinking_budget":1_000}),
        ),
    ];
    for (dialect, budget_tokens, effort, expected) in cases {
        let (base, server) = upstream(sse(fixture(Protocol::OpenaiChat, "text"))).await;
        let mut req = input(Protocol::OpenaiChat, base);
        req.reasoning_effort = effort;
        req.connection.reasoning = Some(ReasoningConfig {
            dialect,
            efforts: vec![effort],
            budget_tokens,
        });
        let (summary, _) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(
            summary.status,
            EndStatus::Completed,
            "{dialect:?} {effort:?}"
        );
        let body = request_body(&requests(&server)[0]);
        for key in [
            "thinking",
            "reasoning_effort",
            "enable_thinking",
            "thinking_budget",
        ] {
            assert_eq!(
                body.get(key),
                expected.get(key),
                "{dialect:?} {effort:?} {key}"
            );
        }
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
async fn streams_text_and_usage_for_every_protocol() {
    for protocol in PROTOCOLS {
        for name in ["text", "chinese", "truncated"] {
            let want = expected(protocol, name);
            let text = want["text"].as_str().unwrap();
            let i = u32::try_from(want["inputTokens"].as_u64().unwrap()).unwrap();
            let o = u32::try_from(want["outputTokens"].as_u64().unwrap()).unwrap();
            let body = fixture(protocol, name);
            let cut = body.len();
            let (base, server) = upstream(sse_cut(&body, cut)).await;
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
            // Only the Responses fixtures report (zero) cache/reasoning details.
            let detail = (protocol == Protocol::OpenaiResponses).then_some(0);
            let expected = Usage {
                input_tokens: Some(i),
                output_tokens: Some(o),
                cache_read_tokens: detail,
                cache_write_tokens: None,
                reasoning_tokens: detail,
            };
            assert_eq!(summary.usage, expected, "{protocol:?} {name}");
            assert!(events.iter().any(|event| matches!(event,
                RunEvent::Usage { input_tokens, output_tokens, cost: None, .. } if *input_tokens == Some(i) && *output_tokens == Some(o)
            )));

            let request = requests(&server).remove(0);
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
        let (base, _server) = upstream(sse_cut(&body, cut)).await;
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
            let (base, _server) = upstream(Reply::status(status, leaky.as_str())).await;
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

fn bearer(raw: &str) -> String {
    raw.lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("authorization: bearer ")
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn unauthorized_first_key_fails_over_to_the_next() {
    let protocol = Protocol::OpenaiChat;
    let (base, server) = script(vec![
        Reply::status(401, r#"{"error":{"message":"bad k1"}}"#),
        sse(fixture(protocol, "text")),
    ])
    .await;
    let mut req = input(protocol, base);
    req.connection.api_keys = vec![
        Zeroizing::new("k1".to_owned()),
        Zeroizing::new("k2".to_owned()),
    ];
    let (summary, _) = run(&runner(5_000, 5_000), req, false).await;
    assert_eq!(summary.status, EndStatus::Completed);
    let auth: Vec<String> = requests(&server).iter().map(|raw| bearer(raw)).collect();
    assert_eq!(auth, ["k1", "k2"]);
}

#[tokio::test]
async fn every_key_unauthorized_fails_with_the_last_reply() {
    let protocol = Protocol::OpenaiChat;
    let (base, server) = upstream(Reply::status(401, r#"{"error":{"message":"bad key"}}"#)).await;
    let mut req = input(protocol, base);
    req.connection.api_keys = vec![
        Zeroizing::new("k1".to_owned()),
        Zeroizing::new("k2".to_owned()),
    ];
    let (_, events) = run(&runner(5_000, 5_000), req, false).await;
    let (end_status, end_code, _) = end(&events);
    assert_eq!(
        (end_status, end_code),
        (EndStatus::Failed, Some(KitErrorCode::Unauthorized))
    );
    let auth: Vec<String> = requests(&server).iter().map(|raw| bearer(raw)).collect();
    assert_eq!(auth, ["k1", "k2"]);
}

#[tokio::test]
async fn first_byte_timeout_fires_before_and_after_headers() {
    for protocol in PROTOCOLS {
        for reply in [Reply::silent(), sse_cut(&fixture(protocol, "text"), 0)] {
            let (base, _server) = upstream(reply).await;
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
        let (base, _server) = upstream(sse_cut(&body, cut)).await;
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
        let (base, _server) = upstream(sse_cut(&body, cut)).await;
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
    let (base, _server) = upstream(Reply::silent()).await;
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
    let (base, _server) = upstream(sse(body[..cut].to_owned())).await;
    let (summary, events) = run(
        &runner(5_000, 5_000),
        input(Protocol::AnthropicMessages, base),
        false,
    )
    .await;
    assert_eq!(end(&events).1, Some(KitErrorCode::Network));
    assert_eq!(summary_text(&summary), "Hello");
}

fn png() -> ImagePart {
    ImagePart {
        mime: "image/png".to_owned(),
        data: "iVBORw0KGgo=".to_owned(),
    }
}

#[tokio::test]
async fn images_are_encoded_per_protocol_for_input_and_history() {
    for protocol in PROTOCOLS {
        let (base, server) = upstream(sse(fixture(protocol, "text"))).await;
        let mut with = input(protocol, base);
        with.images = vec![png()];
        with.history[0] = Message::UserImages {
            text: "earlier".to_owned(),
            images: vec![png()],
        };
        let (summary, _) = run(&runner(5_000, 5_000), with, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let body = request_body(&requests(&server)[0]);
        let list = if protocol == Protocol::OpenaiResponses {
            &body["input"]
        } else {
            &body["messages"]
        };
        let users: Vec<_> = list
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "user")
            .collect();
        let url = "data:image/png;base64,iVBORw0KGgo=";
        let (history, current) = (&users[0]["content"], &users[1]["content"]);
        for (content, text) in [(history, "earlier"), (current, "hi")] {
            let want = match protocol {
                Protocol::OpenaiChat => {
                    serde_json::json!([{"type":"text","text":text},{"type":"image_url","image_url":{"url":url}}])
                }
                Protocol::OpenaiResponses => {
                    serde_json::json!([{"type":"input_text","text":text},{"type":"input_image","image_url":url}])
                }
                Protocol::AnthropicMessages => {
                    serde_json::json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}},{"type":"text","text":text}])
                }
            };
            assert_eq!(content, &want, "{protocol:?}");
        }
    }
}

#[tokio::test]
async fn non_vision_rejects_input_images_offline_and_strips_history_images() {
    let (base, server) = upstream(sse(fixture(Protocol::OpenaiChat, "text"))).await;
    let mut rejected = input(Protocol::OpenaiChat, base);
    rejected.connection.can_vision = false;
    rejected.images = vec![png()];
    let (summary, events) = run(&runner(5_000, 5_000), rejected, false).await;
    assert_eq!(summary.status, EndStatus::Failed);
    assert_eq!(end(&events).1, Some(KitErrorCode::BadRequest));
    assert!(server.seen().is_empty(), "no request may leave");

    let (base, server) = upstream(sse(fixture(Protocol::OpenaiChat, "text"))).await;
    let mut stripped = input(Protocol::OpenaiChat, base);
    stripped.connection.can_vision = false;
    stripped.history[0] = Message::UserImages {
        text: "earlier".to_owned(),
        images: vec![png()],
    };
    let (summary, _) = run(&runner(5_000, 5_000), stripped, false).await;
    assert_eq!(summary.status, EndStatus::Completed);
    let body = request_body(&requests(&server)[0]);
    assert_eq!(body["messages"][1]["content"], "earlier");
    assert_eq!(body["messages"][3]["content"], "hi");
}

#[tokio::test]
async fn last_context_is_estimated_without_reports_and_unknown_after_cancellation() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (base, _server) = upstream(sse(body.to_owned())).await;
    let mut request = input(Protocol::OpenaiChat, base.clone());
    request.connection.context_window = Some(8192);
    request.connection.endpoint_id = Some("endpoint".to_owned());
    let (summary, _) = run(&runner(5000, 5000), request, false).await;
    let context = summary.context_usage.unwrap();
    assert_eq!(context.source, aether_support::ContextSource::Estimated);
    assert!(context.tokens.is_some_and(|tokens| tokens > 0));
    assert_eq!(context.context_window, Some(8192));
    assert_eq!(context.endpoint_id.as_deref(), Some("endpoint"));
    let (cancelled, _) = run(&runner(5000, 5000), input(Protocol::OpenaiChat, base), true).await;
    let context = cancelled.context_usage.unwrap();
    assert_eq!(context.source, aether_support::ContextSource::Unknown);
    assert_eq!(context.tokens, None);
}
