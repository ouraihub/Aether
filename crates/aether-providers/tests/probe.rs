//! `Store::probe` against loopback upstreams (docs/11 §5).
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_providers::{
    KitErrorCode, ProbeError, ProbeErrorKind, ProbeResult, ProbeTarget, Protocol, Store,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const KEY: &str = "sk-test-0123456789abcdefgh";

/// Request heads (lower-cased) seen by an upstream.
type Seen = Arc<tokio::sync::Mutex<Vec<String>>>;

/// A loopback upstream answering every request with `response`; `None` never answers.
async fn upstream(response: Option<Vec<u8>>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Seen::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut head = Vec::new();
            let mut buf = [0_u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            log.lock()
                .await
                .push(String::from_utf8_lossy(&head).to_lowercase());
            match &response {
                Some(bytes) => {
                    // The client may hang up early (oversized body); that is the point.
                    let _ = socket.write_all(bytes).await;
                    let _ = socket.shutdown().await;
                }
                None => tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    drop(socket);
                })
                .await
                .unwrap(),
            }
        }
    });
    (base, seen)
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

fn draft(base_url: &str, protocol: Protocol, api_key: Option<&str>) -> ProbeTarget {
    ProbeTarget::Draft {
        base_url: base_url.to_owned(),
        protocol,
        api_key: api_key.map(str::to_owned),
        endpoint_id: None,
    }
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
        let (base, _) = upstream(http("200 OK", body)).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
        assert_eq!(result.error, None, "{body}");
        assert_eq!(result.probe_url, format!("{base}/v1/models"));
        assert_eq!(ids(&result), ["claude-sonnet-4-5", "gpt-4o"], "{body}");
        let claude = &result.models[0];
        assert_eq!(claude.group.as_deref(), Some("claude"));
        assert_eq!(claude.model.supports_tools, Some(true));
    }
}

#[tokio::test]
async fn maps_upstream_statuses_with_hints_and_masked_text() {
    let (_dir, store) = store();
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
        let (base, _) = upstream(http(status, &body)).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, Some(KEY))).await;
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
    let (base, _) = upstream(http("404 Not Found", "")).await;
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
        let (base, _) = upstream(http("200 OK", body)).await;
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
        let (base, _) = upstream(response).await;
        let result = probe(&store, draft(&base, Protocol::OpenaiChat, None)).await;
        assert_eq!(error(&result).kind, ProbeErrorKind::InvalidBody);
    }
    // At exactly the cap the same shape is accepted.
    let fits = format!(r#"["{}"]"#, "a".repeat(limit - 4));
    let (base, _) = upstream(http("200 OK", &fits)).await;
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
    // Clearing the key in the draft (`""`) is no key either.
    let result = probe(
        &store,
        draft("https://api.example.com", Protocol::OpenaiChat, Some("")),
    )
    .await;
    assert_eq!(error(&result).kind, ProbeErrorKind::MissingKey);
}

#[tokio::test]
async fn blocked_addresses_are_reported() {
    let (_dir, store) = store();
    // A private IP literal is refused by `check_url`.
    let result = probe(
        &store,
        draft("https://10.0.0.5", Protocol::OpenaiChat, Some(KEY)),
    )
    .await;
    assert_eq!(error(&result).kind, ProbeErrorKind::BlockedAddress);
    // `localhost.` is a domain resolving to loopback: refused at connect time.
    let (base, seen) = upstream(http("200 OK", r#"["m"]"#)).await;
    let port = base.rsplit(':').next().unwrap();
    let url = format!("https://localhost.:{port}");
    let result = probe(&store, draft(&url, Protocol::OpenaiChat, Some(KEY))).await;
    assert_eq!(error(&result).kind, ProbeErrorKind::BlockedAddress);
    assert!(seen.lock().await.is_empty());
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
    let (base, _) = upstream(None).await;
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
    assert_eq!(seen.lock().await.len(), 1, "the request was in flight");
}

#[cfg(windows)]
#[tokio::test]
async fn saved_and_draft_targets_send_the_stored_key() {
    let (_dir, store) = store();
    let (base, seen) = upstream(http("200 OK", r#"{"data":[{"id":"claude-sonnet-4-5"}]}"#)).await;
    let saved = store
        .save(
            serde_json::from_value(serde_json::json!({ "name": "Acme", "endpoints": [{
                "slug": "main", "baseUrl": base, "protocol": "anthropic-messages",
                "apiKey": KEY, "models": [] }] }))
            .unwrap(),
        )
        .unwrap();
    let endpoint_id = saved[0].endpoints[0].id.clone();

    let saved_target = ProbeTarget::Saved {
        endpoint_id: endpoint_id.clone(),
    };
    assert_eq!(
        ids(&probe(&store, saved_target).await),
        ["claude-sonnet-4-5"]
    );
    // Edited address, key field untouched: the stored key of `endpointId` is reused.
    let (edited, edited_seen) = upstream(http("200 OK", r#"["m"]"#)).await;
    let reuse = ProbeTarget::Draft {
        base_url: format!("{edited}/v1/"),
        protocol: Protocol::OpenaiChat,
        api_key: None,
        endpoint_id: Some(endpoint_id),
    };
    assert_eq!(ids(&probe(&store, reuse).await), ["m"]);

    let anthropic = seen.lock().await[0].clone();
    assert!(anthropic.starts_with("get /v1/models "), "{anthropic}");
    for header in [
        format!("authorization: bearer {KEY}"),
        format!("x-api-key: {KEY}"),
        "anthropic-version: 2023-06-01".to_owned(),
    ] {
        assert!(anthropic.contains(&header.to_lowercase()), "{header}");
    }
    let openai = edited_seen.lock().await[0].clone();
    assert!(openai.contains(&format!("authorization: bearer {}", KEY.to_lowercase())));
    assert!(!openai.contains("x-api-key"));
}
