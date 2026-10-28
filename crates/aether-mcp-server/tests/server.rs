//! Real loopback HTTP against the server (docs/13 §6–§8, MS5-1).
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::io::Read as _;
use std::sync::Arc;
use std::time::Duration;

use aether_mcp_server::{
    CallContext, DEFAULT_MAX_REQUEST_BYTES, ExposedTool, KitErrorCode, McpServer, ScopeSpec,
    ServerConfig,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

fn free_port() -> u16 {
    // Not `bind(0)` in the server: find a free port here and hand it over.
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn tool(name: &'static str, scope: &'static str) -> ExposedTool {
    ExposedTool {
        name,
        description: "test tool",
        input_schema: json!({ "type": "object" }),
        scope,
        read_only: scope == "read",
        destructive: false,
        idempotent: true,
        max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
        handler: Arc::new(move |args: Value, ctx: CallContext| {
            Box::pin(
                async move { Ok(json!({ "tool": name, "token": ctx.token_id, "args": args })) },
            )
        }),
    }
}

fn server(root: &std::path::Path, port: u16) -> McpServer {
    McpServer::new(
        root,
        ServerConfig {
            default_port: port,
            token_prefix: "aether",
        },
        vec![
            ScopeSpec {
                id: "read",
                label_key: "scope.read",
                destructive: false,
            },
            ScopeSpec {
                id: "write",
                label_key: "scope.write",
                destructive: true,
            },
        ],
        vec![tool("notes_read", "read"), tool("notes_write", "write")],
    )
    .unwrap()
}

struct Reply {
    status: u16,
    body: Value,
}

async fn post(port: u16, token: Option<&str>, origin: Option<&str>, body: &Value) -> Reply {
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .json(body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    Reply {
        status,
        body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
    }
}

async fn rpc(port: u16, token: &str, method: &str, params: Value) -> Reply {
    post(
        port,
        Some(token),
        None,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
    )
    .await
}

async fn tool_names(port: u16, token: &str) -> Vec<String> {
    let reply = rpc(port, token, "tools/list", json!({})).await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    reply.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn only_the_sha256_of_a_token_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(dir.path(), free_port());
    let issued = server.issue_token("cli", &["read"]).unwrap();
    assert!(issued.token.starts_with("aether_"));
    // 32 random bytes → 43 base64url characters.
    assert_eq!(issued.token.len(), "aether_".len() + 43);

    let db = rusqlite::Connection::open(dir.path().join("mcp-server.db")).unwrap();
    let (hash, dump): (String, String) = db
        .query_row(
            "SELECT token_hash, id || name || scopes || created_at FROM tokens",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let expected: String = Sha256::digest(issued.token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hash, expected);
    let secret = &issued.token["aether_".len()..];
    assert!(!dump.contains(secret) && !hash.contains(secret));

    let listed = server.tokens().unwrap();
    assert_eq!(listed, vec![issued.record]);
    assert_eq!(
        server.issue_token("x", &["admin"]).err().unwrap().code(),
        KitErrorCode::BadRequest
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn revoked_token_is_rejected_on_the_next_request() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let server = server(dir.path(), port);
    server.set_enabled_scopes(&["read"]).unwrap();
    let issued = server.issue_token("cli", &["read"]).unwrap();
    assert!(server.start().await.unwrap().running);

    assert_eq!(tool_names(port, &issued.token).await, ["notes_read"]);
    assert!(server.tokens().unwrap()[0].last_used_at.is_some());
    server.revoke_token(&issued.record.id).unwrap();
    let reply = rpc(port, &issued.token, "tools/list", json!({})).await;
    assert_eq!(reply.status, 401);
    assert!(server.tokens().unwrap()[0].revoked_at.is_some());

    assert_eq!(
        rpc(port, "aether_wrong", "tools/list", json!({}))
            .await
            .status,
        401
    );
    let unauthenticated = post(
        port,
        None,
        None,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(unauthenticated.status, 401);
    assert_eq!(
        server.revoke_token("missing").unwrap_err().code(),
        KitErrorCode::NotFound
    );
    server.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_need_token_scope_and_master_switch() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let server = server(dir.path(), port);
    let with_read = server.issue_token("r", &["read"]).unwrap();
    let without_read = server.issue_token("w", &["write"]).unwrap();
    server.start().await.unwrap();

    for (switch_on, token, allowed) in [
        (true, &with_read, true),
        (false, &with_read, false),
        (true, &without_read, false),
        (false, &without_read, false),
    ] {
        server
            .set_enabled_scopes(if switch_on {
                &["read", "write"]
            } else {
                &["write"]
            })
            .unwrap();
        let reply = rpc(
            port,
            &token.token,
            "tools/call",
            json!({ "name": "notes_read", "arguments": { "q": 1 } }),
        )
        .await;
        if allowed {
            let content = &reply.body["result"]["structuredContent"];
            assert_eq!(content["tool"], "notes_read", "{:?}", reply.body);
            assert_eq!(content["token"], token.record.id.as_str());
            assert_eq!(content["args"], json!({ "q": 1 }));
        } else {
            assert_eq!(
                reply.body["error"]["code"], -32001,
                "switch {switch_on}: {:?}",
                reply.body
            );
        }
    }
    let unknown = rpc(
        port,
        &with_read.token,
        "tools/call",
        json!({ "name": "nope" }),
    )
    .await;
    assert_eq!(unknown.body["error"]["code"], -32004);
    server.set_enabled_scopes(&["read"]).unwrap();
    let big = "x".repeat(DEFAULT_MAX_REQUEST_BYTES);
    let oversized = rpc(
        port,
        &with_read.token,
        "tools/call",
        json!({ "name": "notes_read", "arguments": { "q": big } }),
    )
    .await;
    assert_eq!(
        oversized.body["error"]["code"], -32602,
        "{:?}",
        oversized.body
    );
    server.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_list_shows_only_callable_tools() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let server = server(dir.path(), port);
    let read = server.issue_token("r", &["read"]).unwrap();
    let both = server.issue_token("rw", &["read", "write"]).unwrap();
    server.set_enabled_scopes(&["read", "write"]).unwrap();
    server.start().await.unwrap();

    assert_eq!(tool_names(port, &read.token).await, ["notes_read"]);
    assert_eq!(
        tool_names(port, &both.token).await,
        ["notes_read", "notes_write"]
    );
    server.set_enabled_scopes(&["write"]).unwrap();
    assert!(tool_names(port, &read.token).await.is_empty());
    assert_eq!(tool_names(port, &both.token).await, ["notes_write"]);

    // A real handshake works too.
    let init = rpc(
        port,
        &both.token,
        "initialize",
        json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }),
    )
    .await;
    assert!(
        init.body["result"]["capabilities"]["tools"].is_object(),
        "{:?}",
        init.body
    );
    server.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn non_loopback_origin_is_forbidden() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let server = server(dir.path(), port);
    server.set_enabled_scopes(&["read"]).unwrap();
    let issued = server.issue_token("cli", &["read"]).unwrap();
    server.start().await.unwrap();

    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    for origin in [
        "http://evil.example",
        "http://127.0.0.1.evil.example:80",
        "null",
        "file://",
    ] {
        let reply = post(port, Some(&issued.token), Some(origin), &list).await;
        assert_eq!(reply.status, 403, "{origin}");
    }
    // Rejected before authentication: no token, still 403.
    assert_eq!(
        post(port, None, Some("http://evil.example"), &list)
            .await
            .status,
        403
    );
    for origin in [
        "http://localhost:5173",
        "http://127.0.0.1",
        "https://[::1]:8443",
    ] {
        let reply = post(port, Some(&issued.token), Some(origin), &list).await;
        assert_eq!(reply.status, 200, "{origin}");
    }
    server.stop().await.unwrap();
}

#[tokio::test]
async fn stop_closes_idle_and_inflight_connections_before_returning() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let server = server(dir.path(), port);
        let token = server.issue_token("cli", &["read"]).unwrap();
        server.start().await.unwrap();

        // A completed keep-alive request proves this idle socket was accepted.
        let mut idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        idle.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        let mut buffer = [0; 256];
        while !response.ends_with(b"\r\n\r\nnot found") {
            let count = idle.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            response.extend_from_slice(&buffer[..count]);
        }
        assert!(response.starts_with(b"HTTP/1.1 404 "));

        // 100 Continue proves the authenticated route is waiting for this body.
        let mut inflight = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let headers = format!(
            "POST /mcp HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {}\r\n\
             content-type: application/json\r\naccept: application/json, text/event-stream\r\n\
             content-length: 128\r\nexpect: 100-continue\r\n\r\n",
            token.token
        );
        inflight.write_all(headers.as_bytes()).await.unwrap();
        let mut continued = [0; 25];
        inflight.read_exact(&mut continued).await.unwrap();
        assert_eq!(&continued, b"HTTP/1.1 100 Continue\r\n\r\n");
        inflight.write_all(b"{").await.unwrap();

        let connections = [idle.into_std().unwrap(), inflight.into_std().unwrap()];
        assert!(!server.stop().await.unwrap().running);
        // Do not await socket readiness: that would let detached tasks finish late.
        for mut connection in connections {
            match connection.read(&mut [0; 1]) {
                Ok(count) => assert_eq!(count, 0, "connection still has response data"),
                Err(error) => assert!(
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                    ),
                    "connection not closed when stop returned: {error}"
                ),
            }
        }
        let _rebound = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn occupied_port_is_a_conflict_with_the_port() {
    let dir = tempfile::tempdir().unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = taken.local_addr().unwrap().port();
    let server = server(dir.path(), busy);

    let error = server.start().await.unwrap_err();
    assert_eq!(error.code(), KitErrorCode::Conflict);
    assert_eq!(error.details().port(), Some(busy));
    assert!(!server.status().running);

    // Moving to a free port starts; moving back while running reports the conflict.
    let free = free_port();
    server.set_port(free).await.unwrap();
    let status = server.start().await.unwrap();
    assert_eq!((status.running, status.port), (true, free));
    assert_eq!(status.url, format!("http://127.0.0.1:{free}/mcp"));
    let error = server.set_port(busy).await.unwrap_err();
    assert_eq!(error.details().port(), Some(busy));

    // stop frees the port: it can be bound again right away.
    server.set_port(free).await.unwrap();
    server.start().await.unwrap();
    server.stop().await.unwrap();
    drop(std::net::TcpListener::bind(("127.0.0.1", free)).unwrap());
    assert_eq!(
        server.set_port(80).await.unwrap_err().code(),
        KitErrorCode::BadRequest
    );
}
