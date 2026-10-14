//! Tool calls in the protocol layer: fragment assembly, truncation, history encoding (docs/12 §3).

use aether_support::sse::SseParser;
use aether_support::{Approval, Protocol};
use serde_json::{Value, json};

use super::{PROTOCOLS, Reply, end, fixture, input, run, runner, upstream};
use crate::protocol::{Decoder, Round};
use crate::{EndStatus, Message, ToolCall, ToolSource, ToolSpec, Usage};

fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        args,
    }
}

/// What the `single-tool` / `multi-tool` fixtures ask for.
pub(super) fn expected(protocol: Protocol, name: &str) -> Vec<ToolCall> {
    let a = || json!({ "path": "notes/a.md" });
    let b = || json!({ "path": "notes/b.md" });
    let paris = || json!({ "city": "Paris" });
    match (protocol, name) {
        (Protocol::AnthropicMessages, "single-tool") => vec![call(
            "toolu_01T1x1fJ34qAmk2tNTrN7Up6",
            "get_weather",
            json!({ "city": "Paris", "unit": "celsius" }),
        )],
        (Protocol::AnthropicMessages, "multi-tool") => vec![
            call("toolu_01A09q90qw90lq917835lq9", "read_file", a()),
            call("toolu_01B8x7c6v5b4n3m2a1s2d3f4", "read_file", b()),
        ],
        (Protocol::OpenaiChat, "single-tool") => vec![call(
            "call_DdmO9pD3xa9XTPNJ32zg2hcA",
            "get_weather",
            paris(),
        )],
        (Protocol::OpenaiChat, "multi-tool") => vec![
            call("call_mU2mRxkQ9UOWGhL1xFpQf2Ts", "read_file", a()),
            call("call_x1Fb3m0Zr8e5HhKcVqYtJ4pN", "read_file", b()),
        ],
        // `call_id`, not the `fc_…` item id.
        (Protocol::OpenaiResponses, "single-tool") => {
            vec![call("call_12345xyz", "get_weather", paris())]
        }
        (Protocol::OpenaiResponses, "multi-tool") => vec![
            call("call_aaa111", "read_file", a()),
            call("call_bbb222", "read_file", b()),
        ],
        other => unreachable!("{other:?}"),
    }
}

/// Feeds `body` through the SSE parser and the decoder in `step`-byte chunks.
fn decode(protocol: Protocol, body: &str, step: usize) -> Round {
    let mut parser = SseParser::new();
    let mut decoder = Decoder::new(protocol);
    let mut usage = Usage::default();
    for piece in body.as_bytes().chunks(step) {
        for event in parser.feed(piece) {
            decoder.event(&event.data, &mut usage);
        }
    }
    for event in parser.finish() {
        decoder.event(&event.data, &mut usage);
    }
    assert!(decoder.done, "{protocol:?}");
    decoder.finish()
}

/// Edits a fixture; fails if `from` is absent so a fixture change cannot void a test.
pub(super) fn edit(body: &str, from: &str, to: &str) -> String {
    assert!(body.contains(from), "{from}");
    body.replacen(from, to, 1)
}

#[test]
fn assembles_tool_call_fragments_in_any_chunking() {
    for protocol in PROTOCOLS {
        for name in ["single-tool", "multi-tool"] {
            let body = fixture(protocol, name);
            for step in [1, 7] {
                let round = decode(protocol, &body, step);
                let want = Round {
                    tool_calls: expected(protocol, name),
                    truncated: false,
                };
                assert_eq!(round, want, "{protocol:?} {name} {step}");
            }
        }
    }
}

/// `single-tool` with complete arguments but stopped by the output limit.
pub(super) fn limited(protocol: Protocol) -> String {
    let single = fixture(protocol, "single-tool");
    match protocol {
        Protocol::AnthropicMessages => edit(
            &single,
            r#""stop_reason":"tool_use""#,
            r#""stop_reason":"max_tokens""#,
        ),
        Protocol::OpenaiChat => edit(
            &single,
            r#""finish_reason":"tool_calls""#,
            r#""finish_reason":"length""#,
        ),
        Protocol::OpenaiResponses => edit(
            &edit(
                &single,
                r#""type":"response.completed""#,
                r#""type":"response.incomplete""#,
            ),
            r#""incomplete_details":null,"usage":{"#,
            r#""incomplete_details":{"reason":"max_output_tokens"},"usage":{"#,
        ),
    }
}

#[test]
fn truncation_is_flagged_and_partial_args_never_pass() {
    for protocol in PROTOCOLS {
        let single = fixture(protocol, "single-tool");
        let limited = limited(protocol);
        // The last argument fragment never arrived.
        let cut = match protocol {
            Protocol::AnthropicMessages => edit(
                &single,
                r#""partial_json":": \"celsius\"}""#,
                r#""partial_json":"""#,
            ),
            Protocol::OpenaiChat => edit(&single, r#""arguments":"\"}""#, r#""arguments":"""#),
            Protocol::OpenaiResponses => {
                edit(&single, r#""delta":"Paris\"}""#, r#""delta":"Paris""#)
            }
        };
        let mut partial = expected(protocol, "single-tool");
        partial[0].args = Value::Null;
        for step in [1, 7] {
            let text = decode(protocol, &fixture(protocol, "truncated"), step);
            assert_eq!(
                text,
                Round {
                    tool_calls: Vec::new(),
                    truncated: true
                },
                "{protocol:?}"
            );
            let round = decode(protocol, &limited, step);
            let want = Round {
                tool_calls: expected(protocol, "single-tool"),
                truncated: true,
            };
            assert_eq!(round, want, "{protocol:?} {step}");
            let round = decode(protocol, &cut, step);
            let want = Round {
                tool_calls: partial.clone(),
                truncated: true,
            };
            assert_eq!(round, want, "{protocol:?} {step}");
        }
    }
}

/// JSON body of a raw HTTP request.
pub(super) fn body(raw: &str) -> Value {
    serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn tool_calls_round_trip_through_the_next_request() {
    let schema = json!({ "type": "object", "properties": { "path": { "type": "string" } } });
    for protocol in PROTOCOLS {
        // Round 1: the model asks for two calls.
        let reply = fixture(protocol, "multi-tool");
        let cut = reply.len();
        let (base, seen) = upstream(Reply::Sse { body: reply, cut }).await;
        let mut first = input(protocol, base);
        first.history.clear();
        // One round: only the first request is inspected here; the loop has its own tests.
        first.budget.max_rounds = 1;
        first.tools = vec![ToolSpec {
            name: "read_file".to_owned(),
            description: "Reads a file".to_owned(),
            parameters: schema.clone(),
            approval: Approval::Auto,
            source: ToolSource::Host,
        }];
        let (summary, events) = run(&runner(5_000, 5_000), first, false).await;
        assert_eq!(end(&events).0, EndStatus::Completed, "{protocol:?}");
        let calls = expected(protocol, "multi-tool");
        let assistant = Message::Assistant {
            text: String::new(),
            tool_calls: calls.clone(),
            reasoning: vec![],
        };
        assert_eq!(summary.messages[0], assistant, "{protocol:?}");
        let tools = match protocol {
            Protocol::AnthropicMessages => json!([
                { "name": "read_file", "description": "Reads a file", "input_schema": schema }
            ]),
            Protocol::OpenaiChat => json!([{ "type": "function", "function":
                { "name": "read_file", "description": "Reads a file", "parameters": schema } }]),
            Protocol::OpenaiResponses => json!([{ "type": "function", "name": "read_file",
                "description": "Reads a file", "parameters": schema }]),
        };
        assert_eq!(body(&seen.lock().await[0])["tools"], tools, "{protocol:?}");

        // Round 2: results come back in reverse execution order.
        let reply = fixture(protocol, "text");
        let cut = reply.len();
        let (base, seen) = upstream(Reply::Sse { body: reply, cut }).await;
        let (a, b) = (calls[0].id.as_str(), calls[1].id.as_str());
        let result = |id: &str, text: &str| Message::Tool {
            call_id: id.to_owned(),
            text: text.to_owned(),
        };
        let mut second = input(protocol, base);
        second.history = vec![
            Message::User("hi".to_owned()),
            assistant,
            result(b, "B"),
            result(a, "A"),
        ];
        "next".clone_into(&mut second.input);
        let (_, events) = run(&runner(5_000, 5_000), second, false).await;
        assert_eq!(end(&events).0, EndStatus::Completed, "{protocol:?}");
        let raw = seen.lock().await[0].clone();
        let sent = body(&raw);
        let args = |c: &ToolCall| c.args.to_string();
        let (field, want) = match protocol {
            // `tool_result` blocks follow the `tool_use` order, then the new text.
            Protocol::AnthropicMessages => (
                "messages",
                json!([
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": a, "name": "read_file", "input": calls[0].args },
                        { "type": "tool_use", "id": b, "name": "read_file", "input": calls[1].args },
                    ] },
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": a, "content": "A" },
                        { "type": "tool_result", "tool_use_id": b, "content": "B" },
                        { "type": "text", "text": "next" },
                    ] },
                ]),
            ),
            Protocol::OpenaiChat => (
                "messages",
                json!([
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": "", "tool_calls": [
                        { "id": a, "type": "function",
                          "function": { "name": "read_file", "arguments": args(&calls[0]) } },
                        { "id": b, "type": "function",
                          "function": { "name": "read_file", "arguments": args(&calls[1]) } },
                    ] },
                    { "role": "tool", "tool_call_id": b, "content": "B" },
                    { "role": "tool", "tool_call_id": a, "content": "A" },
                    { "role": "user", "content": "next" },
                ]),
            ),
            // The streamed `call_id` comes back verbatim; the `fc_…` item id never does.
            Protocol::OpenaiResponses => {
                assert!(!raw.contains("fc_aaa111"), "{raw}");
                (
                    "input",
                    json!([
                        { "role": "user", "content": "hi" },
                        { "type": "function_call", "call_id": a, "name": "read_file",
                          "arguments": args(&calls[0]) },
                        { "type": "function_call", "call_id": b, "name": "read_file",
                          "arguments": args(&calls[1]) },
                        { "type": "function_call_output", "call_id": b, "output": "B" },
                        { "type": "function_call_output", "call_id": a, "output": "A" },
                        { "role": "user", "content": "next" },
                    ]),
                )
            }
        };
        let mut got = sent[field].as_array().unwrap().clone();
        got.retain(|m| m["role"] != "system");
        assert_eq!(Value::Array(got), want, "{protocol:?}");
    }
}
