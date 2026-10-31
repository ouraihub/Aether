use super::*;
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
        let (base, _) = upstream(sse(body.to_owned())).await;
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
        let (base, seen) = upstream(sse(fixture(protocol, "text"))).await;
        let mut req = input(protocol, base);
        req.connection.headers = vec![
            ("X-Relay-Tag".to_owned(), "team-a".to_owned()),
            // Rejected at save; skipped here too so the key always wins.
            ("Authorization".to_owned(), "Bearer forged".to_owned()),
        ];
        let (summary, _) = run(&runner(5_000, 5_000), req, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let raw = seen.lock().await[0].to_ascii_lowercase();
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
        let (base, seen) = upstream(sse(fixture(protocol, "text"))).await;
        let (summary, _) = run(&runner(5_000, 5_000), input(protocol, base), false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let body = request_body(&seen.lock().await[0]);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("thinking").is_none());

        let (base, seen) = upstream(sse(fixture(protocol, "text"))).await;
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
        let encoded = request_body(&seen.lock().await[0]);
        match protocol {
            Protocol::AnthropicMessages => assert_eq!(
                encoded["thinking"],
                serde_json::json!({"type":"enabled","budget_tokens":2400})
            ),
            Protocol::OpenaiChat => assert_eq!(encoded["reasoning_effort"], "high"),
            Protocol::OpenaiResponses => assert_eq!(encoded["reasoning"]["effort"], "high"),
        }

        let (base, seen) = upstream(sse(fixture(protocol, "text"))).await;
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
        let body = request_body(&seen.lock().await[0]);
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
        let (base, seen) = upstream(sse(fixture(Protocol::AnthropicMessages, "text"))).await;
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
        let body = request_body(&seen.lock().await[0]);
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
        let (base, seen) = upstream(sse(fixture(Protocol::OpenaiChat, "text"))).await;
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
        let body = request_body(&seen.lock().await[0]);
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

