//! Consumer-observable runtime, cancellation, cache and result contracts (docs/27).

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

use super::*;

#[tokio::test]
async fn completed_results_preserve_nontext_structured_and_empty_content() {
    let payloads = [
        json!({ "content": [
            { "type": "image", "data": "AA==", "mimeType": "image/png" },
            { "type": "audio", "data": "AQ==", "mimeType": "audio/wav" },
            { "type": "resource", "resource": { "uri": "https://example.invalid/data", "text": "raw" } },
            { "type": "vendor-extension", "payload": [null, { "count": 2 }] }
        ], "structuredContent": { "nested": { "values": [1, false, null] } } }),
        json!({ "content": [], "structuredContent": {}, "isError": true }),
        json!({ "content": [] }),
    ];
    for payload in payloads {
        let response = payload.clone();
        let fake = Fake::start(move |seen| match seen.method() {
            "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
            "notifications/initialized" => Reply::Status(202),
            "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
            _ => Reply::Json(result(seen, response.clone())),
        })
        .await;
        let (_dir, store, runtime) = runtime();
        store.save(save("raw", &fake.url)).unwrap();
        let cancel = CancellationToken::new();
        let mut session = runtime.session(&cancel, None).await;
        assert_eq!(
            session.call("mcp_raw_t", json!({}), &cancel).await,
            McpCallResult::Completed {
                content: payload["content"].as_array().unwrap().clone(),
                structured_content: payload.get("structuredContent").cloned(),
                is_error: payload["isError"].as_bool().unwrap_or(false),
            }
        );
    }
}

#[tokio::test]
async fn created_session_keeps_configuration_and_approval_snapshot() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
        _ => Reply::Json(result(seen, json!({ "content": [] }))),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    let id = store.save(save("fixed", &fake.url)).unwrap()[0].id.clone();
    let cancel = CancellationToken::new();
    runtime.refresh(&id, &cancel).await.unwrap();
    let mut session = runtime.session(&cancel, None).await;
    assert_eq!(session.tools()[0].approval, Approval::Ask);

    store.set_tool_approval(&id, "t", Approval::Deny).unwrap();
    let mut changed = save("changed", "http://127.0.0.1:1/unreachable");
    changed.id = Some(id);
    changed.enabled = false;
    changed.trusted = false;
    store.save(changed).unwrap();

    assert_eq!(session.tools()[0].prefixed, "mcp_fixed_t");
    assert_eq!(session.tools()[0].approval, Approval::Ask);
    assert_eq!(
        session.call("mcp_fixed_t", json!({}), &cancel).await,
        McpCallResult::Completed {
            content: Vec::new(),
            structured_content: None,
            is_error: false
        },
    );
    assert!(runtime.session(&cancel, None).await.tools().is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn malformed_dispatched_results_are_unknown_and_are_not_replayed() {
    let invalid = [
        json!(null),
        json!([]),
        json!("result"),
        json!({}),
        json!({ "content": null }),
        json!({ "content": {} }),
        json!({ "content": [null] }),
        json!({ "content": ["text"] }),
        json!({ "content": [{}] }),
        json!({ "content": [{ "type": "" }] }),
        json!({ "content": [{ "type": 1 }] }),
        json!({ "content": [{ "type": "text" }] }),
        json!({ "content": [{ "type": "text", "text": null }] }),
        json!({ "content": [{ "type": "text", "text": 1 }] }),
        json!({ "content": [], "isError": null }),
        json!({ "content": [], "isError": "true" }),
        json!({ "content": [], "structuredContent": null }),
        json!({ "content": [], "structuredContent": [] }),
        json!({ "content": [], "structuredContent": "data" }),
    ];
    for payload in invalid {
        let response = payload.clone();
        let fake = Fake::start(move |seen| match seen.method() {
            "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
            "notifications/initialized" => Reply::Status(202),
            "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
            _ => Reply::Json(result(seen, response.clone())),
        })
        .await;
        let (_dir, store, runtime) = runtime();
        store.save(save("invalid", &fake.url)).unwrap();
        let cancel = CancellationToken::new();
        let mut session = runtime.session(&cancel, None).await;
        assert_eq!(
            session.call("mcp_invalid_t", json!({}), &cancel).await,
            McpCallResult::Unknown {
                reason: "invalid_response".to_owned()
            },
            "{payload}"
        );
        assert_eq!(
            fake.seen()
                .await
                .iter()
                .filter(|seen| seen.method() == "tools/call")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn cancellation_before_call_never_dispatches_or_connects() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
        _ => Reply::Json(result(seen, json!({ "content": [] }))),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    store.save(save("cancel", &fake.url)).unwrap();
    let active = CancellationToken::new();
    let mut connected = runtime.session(&active, None).await;
    let mut lazy = runtime.session(&active, None).await;
    let before = fake.seen().await.len();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    for session in [&mut connected, &mut lazy] {
        assert!(matches!(
            session.call("mcp_cancel_t", json!({}), &cancelled).await,
            McpCallResult::Failed { code, .. } if code == "cancelled"
        ));
    }
    assert_eq!(fake.seen().await.len(), before);
    assert_eq!(
        runtime
            .refresh(&store.list().unwrap()[0].id, &cancelled)
            .await
            .unwrap_err()
            .code(),
        KitErrorCode::Cancelled
    );
    assert!(runtime.session(&cancelled, None).await.tools().is_empty());
    assert_eq!(fake.seen().await.len(), before);
    assert!(matches!(
        connected.call("mcp_cancel_t", json!({}), &active).await,
        McpCallResult::Completed { .. }
    ));
}

#[tokio::test]
async fn shutdown_is_terminal_for_connected_and_lazy_sessions() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
        _ => Reply::Json(result(seen, json!({ "content": [] }))),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    let id = store.save(save("closed", &fake.url)).unwrap()[0].id.clone();
    let cancel = CancellationToken::new();
    let mut connected = runtime.session(&cancel, None).await;
    let mut lazy = runtime.session(&cancel, None).await;
    let before = fake.seen().await.len();
    runtime.shutdown().await;
    runtime.shutdown().await;
    for session in [&mut connected, &mut lazy] {
        assert!(matches!(
            session.call("mcp_closed_t", json!({}), &cancel).await,
            McpCallResult::Failed { code, .. } if code == "cancelled"
        ));
    }
    assert_eq!(
        runtime.refresh(&id, &cancel).await.unwrap_err().code(),
        KitErrorCode::Cancelled
    );
    assert!(runtime.session(&cancel, None).await.tools().is_empty());
    assert_eq!(fake.seen().await.len(), before);
}

#[tokio::test]
async fn shutdown_cancels_dispatched_calls_without_replaying_them() {
    let started = Arc::new(Notify::new());
    let signal = Arc::clone(&started);
    let fake = Fake::start(move |seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
        _ => {
            signal.notify_one();
            Reply::Hang
        }
    })
    .await;
    let (_dir, store, runtime) = runtime();
    store.save(save("active", &fake.url)).unwrap();
    let cancel = CancellationToken::new();
    let mut session = runtime.session(&cancel, None).await;
    let shutdown = async {
        started.notified().await;
        runtime.shutdown().await;
    };
    let (result, ()) = tokio::join!(session.call("mcp_active_t", json!({}), &cancel), shutdown);
    assert_eq!(
        result,
        McpCallResult::Unknown {
            reason: "cancelled".to_owned()
        }
    );
    assert_eq!(
        fake.seen()
            .await
            .iter()
            .filter(|seen| seen.method() == "tools/call")
            .count(),
        1
    );
}

fn age_cache(dir: &tempfile::TempDir) {
    rusqlite::Connection::open(dir.path().join("mcp-client.db"))
        .unwrap()
        .execute("UPDATE tools SET fetched_at = 1", [])
        .unwrap();
}

#[tokio::test]
async fn stale_refresh_outlives_run_cancellation_and_keeps_session_snapshot() {
    let lists = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&lists);
    let started = Arc::new(Notify::new());
    let signal = Arc::clone(&started);
    let release = Arc::new(Notify::new());
    let gate = Arc::clone(&release);
    let fake = Fake::start(move |seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ if counter.fetch_add(1, Ordering::SeqCst) == 0 => {
            Reply::Json(result(seen, json!({ "tools": [tool("old")] })))
        }
        _ => {
            signal.notify_one();
            Reply::Gated(
                Arc::clone(&gate),
                result(seen, json!({ "tools": [tool("old"), tool("new")] })),
            )
        }
    })
    .await;
    let (dir, store, runtime) = runtime();
    let id = store.save(save("stale", &fake.url)).unwrap()[0].id.clone();
    runtime
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap();
    store.set_tool_approval(&id, "old", Approval::Auto).unwrap();
    age_cache(&dir);
    let run = CancellationToken::new();
    let session = tokio::time::timeout(Duration::from_secs(5), runtime.session(&run, None))
        .await
        .unwrap();
    assert_eq!(
        session.tools().len(),
        1,
        "stale cache is immediately usable"
    );
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    run.cancel();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.list().unwrap()[0].tools.len() != 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let cached = store.list().unwrap();
    assert_eq!(
        cached[0]
            .tools
            .iter()
            .map(|t| (t.name.as_str(), t.approval))
            .collect::<Vec<_>>(),
        [("new", Approval::Ask), ("old", Approval::Auto)]
    );
    assert_eq!(
        session.tools().len(),
        1,
        "refresh does not mutate an existing catalogue"
    );
    let next = runtime.session(&CancellationToken::new(), None).await;
    assert_eq!(next.tools().len(), 2);
    runtime.shutdown().await;
}

#[tokio::test]
async fn concurrent_shutdown_drains_pending_stale_refresh_without_replacing_cache() {
    let lists = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&lists);
    let started = Arc::new(Notify::new());
    let signal = Arc::clone(&started);
    let fake = Fake::start(move |seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ if counter.fetch_add(1, Ordering::SeqCst) == 0 => {
            Reply::Json(result(seen, json!({ "tools": [tool("old")] })))
        }
        _ => {
            signal.notify_one();
            Reply::Hang
        }
    })
    .await;
    let (dir, store, runtime) = runtime();
    let id = store.save(save("drain", &fake.url)).unwrap()[0].id.clone();
    runtime
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap();
    store.set_tool_approval(&id, "old", Approval::Deny).unwrap();
    age_cache(&dir);
    let session = runtime.session(&CancellationToken::new(), None).await;
    assert_eq!(session.tools().len(), 1);
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let first = runtime.shutdown();
    let second = runtime.shutdown();
    tokio::pin!(first, second);
    // On this single-thread runtime the refresh cannot finish between these polls.
    // A second shutdown must not report completion just because the first took its handle.
    std::future::poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        assert!(second.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(first, second);
    })
    .await
    .unwrap();
    let before = fake.seen().await.len();
    assert_eq!(store.list().unwrap()[0].tools[0].approval, Approval::Deny);
    assert!(
        runtime
            .session(&CancellationToken::new(), None)
            .await
            .tools()
            .is_empty()
    );
    assert_eq!(
        runtime
            .refresh(&id, &CancellationToken::new())
            .await
            .unwrap_err()
            .code(),
        KitErrorCode::Cancelled
    );
    assert_eq!(fake.seen().await.len(), before);
}

#[tokio::test]
async fn cache_replacement_failure_keeps_last_successful_tools_and_approvals() {
    let lists = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&lists);
    let fake = Fake::start(move |seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ if counter.fetch_add(1, Ordering::SeqCst) == 0 => {
            Reply::Json(result(seen, json!({ "tools": [tool("old")] })))
        }
        _ => Reply::Json(result(seen, json!({ "tools": [tool("old"), tool("new")] }))),
    })
    .await;
    let (dir, store, runtime) = runtime();
    let id = store.save(save("cache", &fake.url)).unwrap()[0].id.clone();
    let cancel = CancellationToken::new();
    runtime.refresh(&id, &cancel).await.unwrap();
    store.set_tool_approval(&id, "old", Approval::Auto).unwrap();
    let db = rusqlite::Connection::open(dir.path().join("mcp-client.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_refresh BEFORE INSERT ON tools BEGIN SELECT RAISE(FAIL, 'fixture cache failure'); END;").unwrap();
    assert_eq!(
        runtime.refresh(&id, &cancel).await.unwrap_err().code(),
        KitErrorCode::Internal
    );
    let cached = store.list().unwrap();
    assert_eq!(cached[0].tools.len(), 1);
    assert_eq!(cached[0].tools[0].name, "old");
    assert_eq!(cached[0].tools[0].approval, Approval::Auto);
    let session = runtime.session(&cancel, None).await;
    assert_eq!(session.tools().len(), 1);
    db.execute_batch("DROP TRIGGER reject_refresh").unwrap();
    let refreshed = runtime.refresh(&id, &cancel).await.unwrap();
    assert_eq!(refreshed.len(), 2);
    assert_eq!(
        refreshed
            .iter()
            .map(|t| (t.name.as_str(), t.approval))
            .collect::<Vec<_>>(),
        [("old", Approval::Auto), ("new", Approval::Ask)]
    );
    assert_eq!(session.tools().len(), 1);
}

#[tokio::test]
async fn nontext_and_structured_payloads_share_the_transport_budget() {
    for payload in [
        json!({ "content": [{ "type": "image", "data": "x".repeat(crate::protocol::MAX_MESSAGE_BYTES), "mimeType": "image/png" }] }),
        json!({ "content": [], "structuredContent": { "data": "x".repeat(crate::protocol::MAX_MESSAGE_BYTES) } }),
    ] {
        let fake = Fake::start(move |seen| match seen.method() {
            "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
            "notifications/initialized" => Reply::Status(202),
            "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
            _ => Reply::Json(result(seen, payload.clone())),
        })
        .await;
        let (_dir, store, runtime) = runtime();
        store.save(save("budget", &fake.url)).unwrap();
        let cancel = CancellationToken::new();
        let mut session = runtime.session(&cancel, None).await;
        assert_eq!(
            session.call("mcp_budget_t", json!({}), &cancel).await,
            McpCallResult::Unknown {
                reason: "budget_exceeded".to_owned()
            }
        );
        assert_eq!(
            fake.seen()
                .await
                .iter()
                .filter(|seen| seen.method() == "tools/call")
                .count(),
            1
        );
    }
}
