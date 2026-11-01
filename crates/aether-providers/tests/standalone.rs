//! Public capabilities consumed without Store, SQLite or native credentials services.
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::time::Duration;

use aether_providers::{
    Model, ProbeErrorKind, Protocol,
    connection::{ConnectionSnapshot, EndpointSnapshot, prepare},
    probe::{ProbeInput, probe},
};
use aether_support::net::AddressPolicy;
use aether_testkit::{Exhausted, Reply, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// A loopback upstream that keeps running while held.
struct Seen(Server);

impl Seen {
    /// Request heads (lower-cased) seen so far.
    fn heads(&self) -> Vec<String> {
        self.0
            .seen()
            .iter()
            .map(|request| {
                let raw = &request.raw;
                let end = raw
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map_or(raw.len(), |at| at + 4);
                String::from_utf8_lossy(&raw[..end]).to_lowercase()
            })
            .collect()
    }
}

fn endpoint(base_url: &str) -> EndpointSnapshot {
    EndpointSnapshot {
        endpoint_id: Some("source".into()),
        base_url: base_url.into(),
        protocol: Protocol::OpenaiResponses,
        api_keys: vec![
            Zeroizing::new("first".into()),
            Zeroizing::new("second".into()),
        ],
        headers: vec![("x-custom".into(), "explicit".into())],
        image_base_url: Some("https://images.example/api/".into()),
        image_edit_base_url: Some("https://edit.example/v1/".into()),
    }
}

fn input(base_url: &str) -> ProbeInput {
    let endpoint = endpoint(base_url);
    ProbeInput {
        base_url: endpoint.base_url,
        protocol: endpoint.protocol,
        api_keys: endpoint.api_keys,
        headers: endpoint.headers,
        policy: AddressPolicy::default(),
    }
}

/// Answers the n-th request with `responses[n]` verbatim, later ones with a 500.
async fn server(responses: Vec<String>) -> (String, Seen) {
    let replies = responses.into_iter().map(Reply::raw).collect();
    let unexpected = Reply::raw(http(500, "unexpected request"));
    let server = Server::script(replies, Exhausted::Reply(unexpected)).await;
    (server.url(), Seen(server))
}

fn http(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn prepares_fixed_keys_overrides_and_effective_model_without_reinferring_reasoning() {
    let model: Model = serde_json::from_value(serde_json::json!({
        "id": "gpt-5", "modelType": "chat", "typeSource": "user", "contextWindow": 10000,
        "maxOutput": 1000, "canVision": true, "supportsTools": true, "reasoning": null,
        "nativeWebSearch": true, "name": "Custom"
    }))
    .unwrap();
    let connection = prepare(ConnectionSnapshot {
        endpoint: endpoint("https://api.example/"),
        model: Some(model),
    })
    .unwrap();
    assert_eq!(connection.base_url, "https://api.example/v1");
    assert_eq!(
        connection.image_base_url.as_deref(),
        Some("https://images.example/api/v1")
    );
    assert_eq!(
        connection.image_edit_base_url.as_deref(),
        Some("https://edit.example/v1")
    );
    assert_eq!(
        connection
            .api_keys
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(connection.reasoning.is_none());
    assert!(connection.native_web_search && connection.can_vision);
    assert_eq!(connection.context_window, Some(10000));
    let again = prepare(ConnectionSnapshot {
        endpoint: endpoint(&connection.base_url),
        model: None,
    })
    .unwrap();
    assert_eq!(again.base_url, connection.base_url);
    assert!(again.model_id.is_none() && again.supports_tools.is_none());
}

#[test]
fn rejects_invalid_explicit_configuration() {
    for headers in [
        vec![("authorization".into(), "override".into())],
        vec![("x-test".into(), "line\r\nbreak".into())],
        vec![
            ("x-test".into(), "one".into()),
            ("X-Test".into(), "two".into()),
        ],
    ] {
        let mut snapshot = endpoint("https://api.example");
        snapshot.headers = headers;
        let error = prepare(ConnectionSnapshot {
            endpoint: snapshot,
            model: None,
        })
        .err()
        .unwrap();
        assert_eq!(error.details().field(), Some("headers"));
    }
    let mut snapshot = endpoint("ftp://api.example");
    snapshot.headers.clear();
    assert!(
        prepare(ConnectionSnapshot {
            endpoint: snapshot,
            model: None
        })
        .is_err()
    );
}

#[tokio::test]
async fn fixed_key_fallback_authentication_parsing_and_no_cross_call_rotation() {
    for status in [401, 403, 429] {
        let (base, seen) = server(vec![
            http(status, "first second"),
            http(200, r#"{"data":[{"id":"gpt-4o"},{"id":"gpt-4o"}]}"#),
        ])
        .await;
        let mut config = input(&base);
        config.protocol = Protocol::AnthropicMessages;
        let result = probe(config, &CancellationToken::new()).await.unwrap();
        assert!(result.error.is_none());
        assert_eq!(result.models.len(), 1);
        let requests = seen.heads();
        assert_eq!(requests.len(), 2);
        for (request, key) in requests.iter().zip(["first", "second"]) {
            assert!(request.starts_with("get /v1/models "));
            assert!(request.contains(&format!("authorization: bearer {key}")));
            assert!(request.contains(&format!("x-api-key: {key}")));
            assert!(request.contains("anthropic-version: 2023-06-01"));
            assert!(request.contains("x-custom: explicit"));
        }
    }
    let (base, seen) = server(vec![http(200, "[\"gpt-4o\"]"), http(200, "[\"gpt-4o\"]")]).await;
    for _ in 0..2 {
        probe(input(&base), &CancellationToken::new())
            .await
            .unwrap();
    }
    assert!(
        seen.heads()
            .iter()
            .all(|request| request.contains("authorization: bearer first"))
    );
}

#[tokio::test]
async fn other_errors_do_not_retry_and_all_attempt_keys_are_redacted() {
    for status in [400, 404, 500] {
        let (base, seen) = server(vec![http(status, "first second")]).await;
        let result = probe(input(&base), &CancellationToken::new())
            .await
            .unwrap();
        let error = result.error.unwrap();
        assert_eq!(error.status, Some(status));
        let message = error.message.unwrap_or_default();
        assert!(!message.contains("first") && !message.contains("second"));
        assert_eq!(seen.heads().len(), 1);
    }
}

#[tokio::test]
async fn probe_and_suggestion_share_policy_cancellation_and_body_limits() {
    let (base, seen) = server(vec![
        http(200, "[\"claude-sonnet-4-5\"]"),
        "HTTP/1.1 400 Response\r\nContent-Length: 4194305\r\nConnection: close\r\n\r\n".into(),
    ])
    .await;
    let result = probe(input(&base), &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.error.is_none());
    assert!(result.suggested_protocol.is_none());
    let requests = seen.heads();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("post /v1/messages "));
    assert!(requests[1].contains("authorization: bearer first"));
    drop(requests);

    let (base, seen) = server(vec![http(401, "retry")]).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(probe(input(&base), &cancel).await.is_err());
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(seen.heads().is_empty());

    let result = probe(input("https://192.168.1.1"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.error.unwrap().kind, ProbeErrorKind::BlockedAddress);
    let mut config = input("https://public.example");
    config.api_keys.clear();
    let result = probe(config, &CancellationToken::new()).await.unwrap();
    assert_eq!(result.error.unwrap().kind, ProbeErrorKind::MissingKey);
}

#[tokio::test]
async fn bounded_model_bodies_and_redirects_do_not_start_extra_requests() {
    let oversized = "x".repeat(4 * 1024 * 1024 + 1);
    let chunked = format!(
        "HTTP/1.1 200 Response\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n",
        oversized.len()
    );
    for response in [
        "HTTP/1.1 200 Response\r\nContent-Length: 4194305\r\nConnection: close\r\n\r\n".into(),
        chunked,
    ] {
        let (base, seen) = server(vec![response]).await;
        let result = probe(input(&base), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.error.unwrap().kind, ProbeErrorKind::InvalidBody);
        assert_eq!(seen.heads().len(), 1);
    }
    let (target, received) = server(vec![http(200, "[\"gpt-4o\"]")]).await;
    let (base, seen) = server(vec![format!("HTTP/1.1 302 Found\r\nLocation: {target}/v1/models\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")]).await;
    let result = probe(input(&base), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.error.unwrap().status, Some(302));
    assert_eq!(seen.heads().len(), 1);
    assert!(received.heads().is_empty());
}

#[tokio::test]
async fn cancellation_drops_inflight_probe_before_another_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let cancel = CancellationToken::new();
    let execute = probe(input(&base), &cancel);
    tokio::pin!(execute);
    let (mut first, _) = tokio::select! {
        accepted = listener.accept() => accepted.unwrap(),
        result = &mut execute => panic!("probe completed before accepting its request: {}", result.is_ok()),
    };
    let mut bytes = [0; 4096];
    tokio::select! {
        read = first.read(&mut bytes) => { assert!(read.unwrap() > 0); },
        result = &mut execute => panic!("probe completed before its request: {}", result.is_ok()),
    }
    cancel.cancel();
    assert!(execute.await.is_err());
    let _ = first.write_all(http(401, "retry").as_bytes()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}
