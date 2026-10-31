use super::*;
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
            assert!(matches!(
                events[events.len() - 2],
                RunEvent::Usage { input_tokens, output_tokens, cost: None, .. } if input_tokens == Some(i) && output_tokens == Some(o)
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
    let (base, seen) = script(vec![
        Reply::Status(401, r#"{"error":{"message":"bad k1"}}"#.to_owned()),
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
    let auth: Vec<String> = seen.lock().await.iter().map(|raw| bearer(raw)).collect();
    assert_eq!(auth, ["k1", "k2"]);
}

#[tokio::test]
async fn every_key_unauthorized_fails_with_the_last_reply() {
    let protocol = Protocol::OpenaiChat;
    let (base, seen) = upstream(Reply::Status(
        401,
        r#"{"error":{"message":"bad key"}}"#.to_owned(),
    ))
    .await;
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
    let auth: Vec<String> = seen.lock().await.iter().map(|raw| bearer(raw)).collect();
    assert_eq!(auth, ["k1", "k2"]);
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

fn png() -> ImagePart {
    ImagePart {
        mime: "image/png".to_owned(),
        data: "iVBORw0KGgo=".to_owned(),
    }
}

#[tokio::test]
async fn images_are_encoded_per_protocol_for_input_and_history() {
    for protocol in PROTOCOLS {
        let (base, seen) = upstream(sse(fixture(protocol, "text"))).await;
        let mut with = input(protocol, base);
        with.images = vec![png()];
        with.history[0] = Message::UserImages {
            text: "earlier".to_owned(),
            images: vec![png()],
        };
        let (summary, _) = run(&runner(5_000, 5_000), with, false).await;
        assert_eq!(summary.status, EndStatus::Completed);
        let body = request_body(&seen.lock().await[0]);
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
    let (base, seen) = upstream(sse(fixture(Protocol::OpenaiChat, "text"))).await;
    let mut rejected = input(Protocol::OpenaiChat, base);
    rejected.connection.can_vision = false;
    rejected.images = vec![png()];
    let (summary, events) = run(&runner(5_000, 5_000), rejected, false).await;
    assert_eq!(summary.status, EndStatus::Failed);
    assert_eq!(end(&events).1, Some(KitErrorCode::BadRequest));
    assert!(seen.lock().await.is_empty(), "no request may leave");

    let (base, seen) = upstream(sse(fixture(Protocol::OpenaiChat, "text"))).await;
    let mut stripped = input(Protocol::OpenaiChat, base);
    stripped.connection.can_vision = false;
    stripped.history[0] = Message::UserImages {
        text: "earlier".to_owned(),
        images: vec![png()],
    };
    let (summary, _) = run(&runner(5_000, 5_000), stripped, false).await;
    assert_eq!(summary.status, EndStatus::Completed);
    let body = request_body(&seen.lock().await[0]);
    assert_eq!(body["messages"][1]["content"], "earlier");
    assert_eq!(body["messages"][3]["content"], "hi");
}

