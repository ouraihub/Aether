//! `Store::probe` against loopback upstreams (docs/11 §5).
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::time::{Duration, Instant};

use aether_providers::{
    KitErrorCode, ProbeError, ProbeErrorKind, ProbeResult, ProbeTarget, Protocol, Store,
};
use aether_testkit::{Exhausted, Reply, Request, Server};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const KEY: &str = "sk-test-0123456789abcdefgh";

/// A loopback upstream that keeps running while held.
struct Seen(Server);

impl Seen {
    /// Request heads (lower-cased) seen so far.
    fn heads(&self) -> Vec<String> {
        self.0.seen().iter().map(head).collect()
    }
}

/// The lower-cased request head, up to and including the blank line.
fn head(request: &Request) -> String {
    let raw = &request.raw;
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw.len(), |at| at + 4);
    String::from_utf8_lossy(&raw[..end]).to_lowercase()
}

/// A loopback upstream answering every request with `response`; `None` stays silent for 60s.
async fn upstream(response: Option<Vec<u8>>) -> (String, Seen) {
    // The client may hang up early (oversized body); the write error is then ignored.
    let reply = match response {
        Some(bytes) => Reply::raw(bytes),
        None => Reply::default().then_sleep(Duration::from_secs(60)),
    };
    let server = Server::script(vec![reply], Exhausted::RepeatLast).await;
    (server.url(), Seen(server))
}

fn http(status: &str, body: &str) -> Option<Vec<u8>> {
    Some(
        format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
}

fn draft(base_url: &str, protocol: Protocol, endpoint_id: Option<&str>) -> ProbeTarget {
    ProbeTarget::Draft {
        base_url: base_url.to_owned(),
        protocol,
        endpoint_id: endpoint_id.map(str::to_owned),
        key_id: None,
    }
}

/// A saved endpoint holding `keys` in order; returns its id.
fn keyed(store: &Store, base_url: &str, keys: &[&str]) -> String {
    let saved = store
        .save(
            serde_json::from_value(serde_json::json!({ "name": "Acme", "endpoints": [{
                "slug": "main", "baseUrl": base_url, "protocol": "anthropic-messages", "models": [] }] }))
            .unwrap(),
        )
        .unwrap();
    let endpoint_id = saved[0].endpoints[0].id.clone();
    for key in keys {
        store
            .save_key(
                serde_json::from_value(
                    serde_json::json!({ "endpointId": endpoint_id, "key": key }),
                )
                .unwrap(),
            )
            .unwrap();
    }
    endpoint_id
}

async fn probe(store: &Store, target: ProbeTarget) -> ProbeResult {
    store
        .probe(target, &CancellationToken::new())
        .await
        .unwrap()
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn ids(result: &ProbeResult) -> Vec<&str> {
    result.models.iter().map(|m| m.model.id.as_str()).collect()
}

fn error(result: &ProbeResult) -> &ProbeError {
    assert!(result.models.is_empty());
    result.error.as_ref().unwrap()
}

#[tokio::test]
async fn parses_the_three_shapes_sorted_and_deduplicated() {
    let (_dir, store) = store();
    for body in [
        r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"claude-sonnet-4-5"},{"id":"gpt-4o"}]}"#,
        r#"{"models":[{"name":"gpt-4o"},{"id":"claude-sonnet-4-5"}]}"#,
        r#"["gpt-4o","claude-sonnet-4-5","gpt-4o"]"#,
    ] {
        let (base, _upstream) = upstream(http("200 OK", body)).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
        assert_eq!(result.error, None, "{body}");
        assert_eq!(result.probe_url, format!("{base}/v1/models"));
        assert_eq!(ids(&result), ["claude-sonnet-4-5", "gpt-4o"], "{body}");
        let claude = &result.models[0];
        assert_eq!(claude.group.as_deref(), Some("claude"));
        assert_eq!(claude.model.supports_tools, Some(true));
    }
}

/// A loopback upstream: `GET …/models` lists `listed`, `POST …/messages` answers `messages`.
async fn relay(listed: &'static str, messages: &'static str) -> (String, Seen) {
    let server = Server::handler(move |request| {
        let wire = if head(request).starts_with("post /v1/messages ") {
            http(messages, r#"{"error":{"type":"invalid_request_error"}}"#)
        } else {
            http("200 OK", listed)
        };
        Reply::raw(wire.unwrap())
    })
    .await;
    (server.url(), Seen(server))
}

#[tokio::test]
async fn suggests_messages_only_when_claude_relay_answers_there() {
    let (_dir, store) = store();
    let claude = r#"{"data":[{"id":"claude-sonnet-5-5"},{"id":"gpt-6-sol"}]}"#;
    // Relay validates the empty request on /messages: switch is suggested, with the key sent.
    let endpoint = keyed(&store, "https://unused.example.com", &[KEY]);
    for status in ["400 Bad Request", "422 Unprocessable Entity"] {
        let (base, seen) = relay(claude, status).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, Some(&endpoint))).await;
        assert_eq!(
            result.suggested_protocol,
            Some(Protocol::AnthropicMessages),
            "{status}"
        );
        let seen = seen.heads();
        let post = seen
            .iter()
            .find(|h| h.starts_with("post /v1/messages "))
            .unwrap();
        assert!(
            post.contains(&format!("x-api-key: {}", KEY.to_lowercase())),
            "{post}"
        );
        assert!(post.contains("anthropic-version: 2023-06-01"), "{post}");
    }
    // No /messages route, or the key is refused there: nothing to suggest.
    for status in [
        "404 Not Found",
        "401 Unauthorized",
        "500 Internal Server Error",
    ] {
        let (base, _upstream) = relay(claude, status).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
        assert_eq!(result.error, None);
        assert_eq!(result.suggested_protocol, None, "{status}");
    }
    // Already on Messages, or no claude model listed: /messages is never asked.
    for (listed, protocol) in [
        (claude, Protocol::AnthropicMessages),
        (r#"{"data":[{"id":"gpt-6-sol"}]}"#, Protocol::OpenaiChat),
    ] {
        let (base, seen) = relay(listed, "400 Bad Request").await;
        let result = probe(&store, draft(&base, protocol, None)).await;
        assert_eq!(result.suggested_protocol, None);
        assert_eq!(seen.heads().len(), 1, "only GET /models");
    }
}

#[tokio::test]
async fn maps_upstream_statuses_with_hints_and_masked_text() {
    let (_dir, store) = store();
    let endpoint = keyed(&store, "https://unused.example.com", &[KEY]);
    let body = format!(r#"{{"error":"bad key {KEY}"}}"#);
    for (status, kind, hint) in [
        (
            "401 Unauthorized",
            ProbeErrorKind::Unauthorized,
            Some("providers.hint.check_key"),
        ),
        (
            "403 Forbidden",
            ProbeErrorKind::Unauthorized,
            Some("providers.hint.check_key"),
        ),
        (
            "404 Not Found",
            ProbeErrorKind::NotFound,
            Some("providers.hint.missing_v1"),
        ),
        ("429 Too Many Requests", ProbeErrorKind::RateLimited, None),
        ("502 Bad Gateway", ProbeErrorKind::Status, None),
    ] {
        let (base, _upstream) = upstream(http(status, &body)).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, Some(&endpoint))).await;
        let error = error(&result);
        assert_eq!(error.kind, kind, "{status}");
        assert_eq!(error.status, Some(status[..3].parse().unwrap()));
        assert_eq!(error.hint.as_deref(), hint, "{status}");
        let message = error.message.as_deref().unwrap();
        assert!(
            message.contains("bad key") && !message.contains(KEY),
            "{message}"
        );
    }
    // A 404 on an address that already names /v1 is not the missing-v1 mistake.
    let (base, _upstream) = upstream(http("404 Not Found", "")).await;
    let result = probe(
        &store,
        draft(&format!("{base}/v1"), Protocol::OpenaiChat, None),
    )
    .await;
    assert_eq!(error(&result).hint, None);
    assert_eq!(error(&result).message, None);
}

#[tokio::test]
async fn invalid_and_empty_bodies_are_invalid_body() {
    let (_dir, store) = store();
    for body in [
        "<html>",
        r#"{"data":[]}"#,
        "[]",
        r#"{"data":[{"x":1}]}"#,
        "{}",
    ] {
        let (base, _upstream) = upstream(http("200 OK", body)).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
        assert_eq!(error(&result).kind, ProbeErrorKind::InvalidBody, "{body}");
    }
}

#[tokio::test]
async fn bodies_over_4_mib_are_rejected() {
    let (_dir, store) = store();
    let limit = 4 * 1024 * 1024;
    // Valid JSON just over the cap: `["aaa…"]`.
    let big = format!(r#"["{}"]"#, "a".repeat(limit - 3));
    let declared = http("200 OK", &big);
    let mut streamed = b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_vec();
    streamed.extend_from_slice(big.as_bytes());
    for response in [declared, Some(streamed)] {
        let (base, _upstream) = upstream(response).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
        assert_eq!(error(&result).kind, ProbeErrorKind::InvalidBody);
    }
    // At exactly the cap the same shape is accepted.
    let fits = format!(r#"["{}"]"#, "a".repeat(limit - 4));
    let (base, _upstream) = upstream(http("200 OK", &fits)).await;
    let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
    assert_eq!(result.error, None);
    assert_eq!(result.models.len(), 1);
}

#[tokio::test]
async fn missing_key_off_loopback_sends_nothing() {
    let (_dir, store) = store();
    let result = probe(
        &store,
        draft("https://api.example.com", Protocol::OpenaiChat, None),
    )
    .await;
    assert_eq!(result.probe_url, "https://api.example.com/v1/models");
    assert_eq!(error(&result).kind, ProbeErrorKind::MissingKey);
}

#[tokio::test]
async fn unauthorized_keys_fail_over_and_key_id_pins_one() {
    let (_dir, store) = store();
    let (base, seen) = upstream(http("401 Unauthorized", "{}")).await;
    let endpoint = keyed(&store, &base, &["k1", "k2"]);
    let saved = ProbeTarget::Saved {
        endpoint_id: endpoint.clone(),
    };
    assert_eq!(
        error(&probe(&store, saved).await).kind,
        ProbeErrorKind::Unauthorized
    );
    let auth = |seen: &[String]| -> Vec<String> {
        seen.iter()
            .filter_map(|head| {
                head.lines()
                    .find_map(|l| l.strip_prefix("authorization: bearer ").map(str::to_owned))
            })
            .collect()
    };
    assert_eq!(auth(&seen.heads()), ["k1", "k2"]);

    let key_id = store.list().unwrap()[0].endpoints[0].keys[1].id.clone();
    let pinned = ProbeTarget::Draft {
        base_url: base.clone(),
        protocol: Protocol::OpenaiChat,
        endpoint_id: Some(endpoint.clone()),
        key_id: Some(key_id),
    };
    probe(&store, pinned).await;
    assert_eq!(auth(&seen.heads())[2..], ["k2"]);
    // Only Saved consumed a rotation reservation; probing a pinned key did not.
    assert_eq!(
        store.endpoint_snapshot(&endpoint).unwrap().api_keys[0].as_str(),
        "k2"
    );
    let saved = store
        .save(
            serde_json::from_value(serde_json::json!({
                "name": "Other", "endpoints": [{
                    "slug": "other", "baseUrl": base, "protocol": "openai-chat", "models": []
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    let other = saved
        .iter()
        .find(|provider| provider.name == "Other")
        .unwrap()
        .endpoints[0]
        .id
        .clone();
    assert_ne!(other, endpoint);
    let saved = store
        .save_key(
            serde_json::from_value(serde_json::json!({
                "endpointId": other, "key": "foreign"
            }))
            .unwrap(),
        )
        .unwrap();
    let other_key = saved
        .iter()
        .flat_map(|provider| &provider.endpoints)
        .find(|saved| saved.id == other)
        .unwrap()
        .keys[0]
        .id
        .clone();
    let wrong_owner = ProbeTarget::Draft {
        base_url: base.clone(),
        protocol: Protocol::OpenaiChat,
        endpoint_id: Some(endpoint.clone()),
        key_id: Some(other_key),
    };
    assert_eq!(
        store
            .probe(wrong_owner, &CancellationToken::new())
            .await
            .unwrap_err()
            .details()
            .entity(),
        Some("key")
    );
    let foreign = ProbeTarget::Draft {
        base_url: base,
        protocol: Protocol::OpenaiChat,
        endpoint_id: Some(endpoint),
        key_id: Some("nope".to_owned()),
    };
    let err = store
        .probe(foreign, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(err.details().entity(), Some("key"));
}

#[tokio::test]
async fn blocked_addresses_are_reported() {
    let (_dir, store) = store();
    // A private IP literal is refused by `check_url`.
    let endpoint = keyed(&store, "https://unused.example.com", &[KEY]);
    let result = probe(
        &store,
        draft("https://10.0.0.5", Protocol::OpenaiChat, Some(&endpoint)),
    )
    .await;
    assert_eq!(error(&result).kind, ProbeErrorKind::BlockedAddress);
    // `localhost.` is a domain resolving to loopback: refused at connect time.
    let (base, seen) = upstream(http("200 OK", r#"["m"]"#)).await;
    let port = base.rsplit(':').next().unwrap();
    let url = format!("https://localhost.:{port}");
    let result = probe(&store, draft(&url, Protocol::OpenaiChat, Some(&endpoint))).await;
    assert_eq!(error(&result).kind, ProbeErrorKind::BlockedAddress);
    assert!(seen.heads().is_empty());
}

#[tokio::test]
async fn refused_connection_is_connect_with_network_hint() {
    let (_dir, store) = store();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
    let error = error(&result);
    assert_eq!(error.kind, ProbeErrorKind::Connect);
    assert_eq!(error.hint.as_deref(), Some("providers.hint.check_network"));
}

#[tokio::test]
async fn silent_upstream_times_out_after_20s() {
    let (_dir, store) = store();
    let (base, _upstream) = upstream(None).await;
    let started = Instant::now();
    let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
    assert_eq!(error(&result).kind, ProbeErrorKind::Timeout);
    assert!(started.elapsed() >= Duration::from_secs(19));
}

#[tokio::test]
async fn cancel_aborts_a_pending_request() {
    let (_dir, store) = store();
    let (base, seen) = upstream(None).await;
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let err = store
        .probe(draft(&base, Protocol::OpenaiChat, None), &cancel)
        .await
        .unwrap_err();
    assert_eq!(err.code(), KitErrorCode::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(seen.heads().len(), 1, "the request was in flight");
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn saved_and_draft_targets_send_the_stored_key() {
    let (_dir, store) = store();
    let (base, seen) = upstream(http("200 OK", r#"{"data":[{"id":"claude-sonnet-4-5"}]}"#)).await;
    let endpoint_id = keyed(&store, &base, &[KEY]);

    let saved_target = ProbeTarget::Saved {
        endpoint_id: endpoint_id.clone(),
    };
    assert_eq!(
        ids(&probe(&store, saved_target).await),
        ["claude-sonnet-4-5"]
    );
    // Edited address: the stored keys of `endpointId` are reused.
    let (edited, edited_seen) = upstream(http("200 OK", r#"["m"]"#)).await;
    let reuse = draft(
        &format!("{edited}/v1/"),
        Protocol::OpenaiChat,
        Some(&endpoint_id),
    );
    assert_eq!(ids(&probe(&store, reuse).await), ["m"]);

    let anthropic = seen.heads()[0].clone();
    assert!(anthropic.starts_with("get /v1/models "), "{anthropic}");
    for header in [
        format!("authorization: bearer {KEY}"),
        format!("x-api-key: {KEY}"),
        "anthropic-version: 2023-06-01".to_owned(),
    ] {
        assert!(anthropic.contains(&header.to_lowercase()), "{header}");
    }
    let openai = edited_seen.heads()[0].clone();
    assert!(openai.contains(&format!("authorization: bearer {}", KEY.to_lowercase())));
    assert!(!openai.contains("x-api-key"));
}

#[tokio::test]
async fn saved_and_draft_targets_send_the_stored_custom_headers() {
    let (_dir, store) = store();
    let (base, seen) = upstream(http("200 OK", r#"["m"]"#)).await;
    let saved = store
        .save(
            serde_json::from_value(serde_json::json!({ "name": "Acme", "endpoints": [{
                "slug": "main", "baseUrl": base, "protocol": "openai-chat", "models": [],
                "headers": { "X-Relay-Tag": "team-a" } }] }))
            .unwrap(),
        )
        .unwrap();
    let endpoint_id = saved[0].endpoints[0].id.clone();
    let saved_target = ProbeTarget::Saved {
        endpoint_id: endpoint_id.clone(),
    };
    assert_eq!(ids(&probe(&store, saved_target).await), ["m"]);
    let reuse = draft(&base, Protocol::OpenaiChat, Some(&endpoint_id));
    assert_eq!(ids(&probe(&store, reuse).await), ["m"]);
    let fresh = draft(&base, Protocol::OpenaiChat, None);
    assert_eq!(ids(&probe(&store, fresh).await), ["m"]);
    let seen = seen.heads();
    assert!(seen[0].contains("x-relay-tag: team-a"), "{}", seen[0]);
    assert!(seen[1].contains("x-relay-tag: team-a"), "{}", seen[1]);
    assert!(!seen[2].contains("x-relay-tag"), "{}", seen[2]);
}
