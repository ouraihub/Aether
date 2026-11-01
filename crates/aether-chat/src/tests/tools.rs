//! Tool calls in the protocol layer: fragment assembly, truncation, history encoding (docs/12 §3).

use aether_support::sse::SseParser;
use aether_support::{Approval, Protocol};
use serde_json::{Value, json};

use super::{PROTOCOLS, end, fixture, input, requests, run, runner, sse_cut, upstream};
use crate::protocol::{Decoder, Round};
use crate::{EndStatus, Message, ToolCall, ToolEffect, ToolSource, ToolSpec, Usage};

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
                    reasoning: Vec::new(),
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
                    truncated: true,
                    reasoning: Vec::new(),
                },
                "{protocol:?}"
            );
            let round = decode(protocol, &limited, step);
            let want = Round {
                tool_calls: expected(protocol, "single-tool"),
                truncated: true,
                reasoning: Vec::new(),
            };
            assert_eq!(round, want, "{protocol:?} {step}");
            let round = decode(protocol, &cut, step);
            let want = Round {
                tool_calls: partial.clone(),
                truncated: true,
                reasoning: Vec::new(),
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
        let (base, server) = upstream(sse_cut(&reply, cut)).await;
        let mut first = input(protocol, base);
        first.history.clear();
        // One round: only the first request is inspected here; the loop has its own tests.
        first.budget.max_rounds = 1;
        first.tools = vec![ToolSpec {
            name: "read_file".to_owned(),
            description: "Reads a file".to_owned(),
            parameters: schema.clone(),
            approval: Approval::Auto,
            effect: ToolEffect::Read,
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
        assert_eq!(body(&requests(&server)[0])["tools"], tools, "{protocol:?}");

        // Round 2: results come back in reverse execution order.
        let reply = fixture(protocol, "text");
        let cut = reply.len();
        let (base, server) = upstream(sse_cut(&reply, cut)).await;
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
        let raw = requests(&server).remove(0);
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

#[tokio::test]
async fn native_reasoning_is_displayed_retained_and_replayed_only_for_its_identity() {
    use crate::RunEvent;

    for protocol in PROTOCOLS {
        let (events, native) = match protocol {
            Protocol::AnthropicMessages => {
                let thinking = json!({"type":"thinking","thinking":"Let me think.","signature":"private-signature"});
                let redacted = json!({"type":"redacted_thinking","data":"private-redacted"});
                (
                    vec![
                        json!({"type":"content_block_start","index":10,"content_block":{"type":"thinking","thinking":"Let "}}),
                        json!({"type":"content_block_delta","index":10,"delta":{"type":"thinking_delta","thinking":"me think."}}),
                        json!({"type":"content_block_delta","index":10,"delta":{"type":"signature_delta","signature":"private-"}}),
                        json!({"type":"content_block_delta","index":10,"delta":{"type":"signature_delta","signature":"signature"}}),
                        json!({"type":"content_block_stop","index":10}),
                        json!({"type":"content_block_start","index":11,"content_block":redacted}),
                        json!({"type":"content_block_stop","index":11}),
                    ],
                    vec![thinking, redacted],
                )
            }
            Protocol::OpenaiChat => (
                vec![
                    json!({"choices":[{"delta":{"reasoning_content":"Let "}}]}),
                    json!({"choices":[{"delta":{"reasoning_content":"me think.","content":"Answer"}}]}),
                ],
                vec![json!("Let me think.")],
            ),
            Protocol::OpenaiResponses => {
                let item = json!({"type":"reasoning","id":"rs_private","summary":[{"type":"summary_text","text":"Let me think."}],"encrypted_content":"private-encrypted"});
                (
                    vec![
                        json!({"type":"response.reasoning_summary_text.delta","delta":"Let me think."}),
                        json!({"type":"response.output_item.done","item":item}),
                    ],
                    vec![item],
                )
            }
        };
        let prefix: String = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect();
        let response = prefix.clone() + &fixture(protocol, "single-tool");
        let (base, server) = upstream(sse_cut(&response, response.len())).await;
        let mut first = input(protocol, base);
        first.connection.endpoint_id = Some("origin".to_owned());
        first.history.clear();
        first.budget.max_rounds = 1;
        let (summary, events) = run(&runner(5_000, 5_000), first, false).await;
        assert_eq!(summary.status, EndStatus::Completed, "{protocol:?}");
        let display: String = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::ReasoningDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(display, "Let me think.", "{protocol:?}");
        assert!(!serde_json::to_string(&events).unwrap().contains("private-"));
        let Message::Assistant {
            reasoning,
            tool_calls,
            text,
        } = &summary.messages[0]
        else {
            panic!("missing assistant")
        };
        assert_eq!(*tool_calls, expected(protocol, "single-tool"));
        if protocol == Protocol::OpenaiChat {
            assert!(text.contains("Answer"));
        }
        assert_eq!(
            reasoning
                .iter()
                .map(|block| block.value.clone())
                .collect::<Vec<_>>(),
            native
        );
        for block in reasoning {
            assert_eq!(block.protocol, protocol);
            assert_eq!(block.endpoint_id.as_deref(), Some("origin"));
            assert_eq!(block.model_id.as_deref(), Some("model-x"));
        }

        let terminal = match protocol {
            Protocol::AnthropicMessages => json!({"type":"message_stop"}),
            Protocol::OpenaiChat => json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            Protocol::OpenaiResponses => json!({"type":"response.completed","response":{}}),
        };
        let completed = format!("{prefix}data: {terminal}\n\n");
        let (base, _final_server) = upstream(sse_cut(&completed, completed.len())).await;
        let mut final_input = input(protocol, base);
        final_input.connection.endpoint_id = Some("origin".to_owned());
        final_input.history.clear();
        let (final_summary, _) = run(&runner(5_000, 5_000), final_input, false).await;
        assert_eq!(final_summary.status, EndStatus::Completed);
        let [
            Message::Assistant {
                reasoning: final_reasoning,
                tool_calls: final_calls,
                ..
            },
        ] = final_summary.messages.as_slice()
        else {
            panic!("missing reasoning-only completed assistant")
        };
        assert!(final_calls.is_empty());
        assert_eq!(final_reasoning, reasoning);

        for mismatch in ["none", "endpoint", "model", "protocol"] {
            let target = if mismatch == "protocol" {
                if protocol == Protocol::OpenaiChat {
                    Protocol::OpenaiResponses
                } else {
                    Protocol::OpenaiChat
                }
            } else {
                protocol
            };
            let (next_base, next_server) = upstream(super::sse(fixture(target, "text"))).await;
            let mut next = input(target, next_base);
            next.history = summary.messages.clone();
            next.connection.endpoint_id = Some(
                if mismatch == "endpoint" {
                    "other"
                } else {
                    "origin"
                }
                .to_owned(),
            );
            if mismatch == "model" {
                next.connection.model_id = Some("other-model".to_owned());
            }
            let (replayed, _) = run(&runner(5_000, 5_000), next, false).await;
            assert_eq!(replayed.status, EndStatus::Completed);
            let request = body(&requests(&next_server)[0]);
            let sent = &request[if target == Protocol::OpenaiResponses {
                "input"
            } else {
                "messages"
            }];
            let blocks: Vec<Value> = match target {
                Protocol::AnthropicMessages => sent
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|message| message["content"].as_array().into_iter().flatten())
                    .filter(|block| {
                        block["type"] == "thinking" || block["type"] == "redacted_thinking"
                    })
                    .cloned()
                    .collect(),
                Protocol::OpenaiChat => sent
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|message| message.get("reasoning_content").cloned())
                    .collect(),
                Protocol::OpenaiResponses => sent
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| item["type"] == "reasoning")
                    .cloned()
                    .collect(),
            };
            assert_eq!(
                blocks,
                if mismatch == "none" {
                    native.clone()
                } else {
                    Vec::new()
                },
                "{protocol:?} {mismatch}"
            );
        }
        if protocol == Protocol::OpenaiResponses {
            let request = body(&requests(&server)[0]);
            assert_eq!(request["store"], false);
            assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
        }
    }
}

#[test]
fn responses_terminal_reasoning_replaces_items_and_never_displays_encrypted_content() {
    use crate::protocol::Decoded;

    let mut decoder =
        Decoder::new(Protocol::OpenaiResponses).with_identity(Some("origin"), Some("model-x"));
    let mut usage = Usage::default();
    let initial =
        json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"initial"});
    let final_item = json!({"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"public"}],"encrypted_content":"final-secret"});
    let terminal_only =
        json!({"type":"reasoning","id":"rs_2","encrypted_content":"another-secret","summary":[]});
    for event in [
        json!({"type":"response.output_item.done","item":initial}),
        json!({"type":"response.completed","response":{"output":[final_item, terminal_only]}}),
    ] {
        assert!(matches!(
            decoder.event(&event.to_string(), &mut usage),
            Decoded::Skip
        ));
    }
    let round = decoder.finish();
    assert_eq!(
        round
            .reasoning
            .iter()
            .map(|block| block.value.clone())
            .collect::<Vec<_>>(),
        vec![final_item, terminal_only]
    );
}
