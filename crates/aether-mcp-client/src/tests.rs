//! Behaviour against a local fake MCP server (the shared scripted loopback upstream).
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aether_support::KitErrorCode;
use aether_testkit::{Gate, Request, Server};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::{
    Approval, CancellationToken, McpCallResult, McpRuntime, McpTransport, ServerSave, ServerSecret,
    Store,
};

mod behavior;
mod stdio;

/// What the fake server saw, with the JSON-RPC body decoded.
#[derive(Clone, Debug)]
struct Seen {
    target: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

impl Seen {
    fn new(request: &Request) -> Self {
        Self {
            target: request.target.clone(),
            headers: request.headers.iter().cloned().collect(),
            body: serde_json::from_slice(&request.body).unwrap(),
        }
    }
    fn method(&self) -> &str {
        self.body["method"].as_str().unwrap_or_default()
    }
    fn session(&self) -> Option<&str> {
        self.headers.get("mcp-session-id").map(String::as_str)
    }
}

enum Reply {
    Json(Value),
    Session(String, Value),
    Sse(String),
    Status(u16),
    /// Read the request and never answer.
    Hang,
    /// Answer with this JSON once the gate opens.
    Gated(Gate, Value),
}

impl Reply {
    fn wire(self) -> aether_testkit::Reply {
        use aether_testkit::Reply as Wire;
        match self {
            Self::Json(body) => Wire::json(body.to_string()),
            Self::Session(id, body) => Wire::http(
                200,
                &[
                    ("content-type", "application/json"),
                    ("mcp-session-id", &id),
                ],
                body.to_string(),
            ),
            Self::Sse(body) => Wire::http(200, &[("content-type", "text/event-stream")], body),
            Self::Status(status) => Wire::http(status, &[("content-type", "text/plain")], ""),
            Self::Hang => Wire::silent(),
            Self::Gated(gate, body) => {
                let body = body.to_string();
                Wire::close().then_wait(&gate).then_raw(format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                ))
            }
        }
    }
}

/// A fake MCP endpoint at `/mcp`; every connection is answered by `handler`, concurrently.
struct Fake {
    url: String,
    server: Server,
}

impl Fake {
    async fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Self {
        let server = Server::handler(move |request| handler(&Seen::new(request)).wire()).await;
        Self {
            url: format!("{}/mcp", server.url()),
            server,
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.server.seen().iter().map(Seen::new).collect()
    }
}

fn result(seen: &Seen, result: Value) -> Value {
    let mut message = json!({ "jsonrpc": "2.0", "id": seen.body["id"] });
    message["result"] = result;
    message
}

fn initialized(seen: &Seen, version: &str) -> Value {
    result(
        seen,
        json!({ "protocolVersion": version, "capabilities": {}, "serverInfo": { "name": "fake" } }),
    )
}

fn tool(name: &str) -> Value {
    json!({ "name": name, "description": "d", "inputSchema": { "type": "object" } })
}

fn save(slug: &str, url: &str) -> ServerSave {
    ServerSave {
        id: None,
        slug: slug.to_owned(),
        name: slug.to_owned(),
        transport: McpTransport::Http {
            url: url.to_owned(),
        },
        secret: None,
        enabled: true,
        trusted: true,
        allow_private: false,
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn runtime() -> (tempfile::TempDir, Arc<Store>, McpRuntime) {
    let (dir, store) = store();
    let store = Arc::new(store);
    let runtime = McpRuntime::new(Arc::clone(&store));
    (dir, store, runtime)
}

/// initialize (session `s1`) → initialized → tools/list page 1 (JSON) → page 2 (SSE with an
/// unrelated message first) → tools/call by prefixed name.
#[tokio::test]
async fn discovers_pages_over_json_and_sse_and_reuses_the_session() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Session("s1".to_owned(), initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        "tools/list" if seen.body["params"]["cursor"].is_null() => Reply::Json(result(
            seen,
            json!({ "tools": [tool("alpha")], "nextCursor": "p2" }),
        )),
        "tools/list" => {
            let mut beta = tool("beta");
            beta["annotations"] = json!({ "readOnlyHint": true });
            beta["description"] = json!("x".repeat(2000));
            let mut huge = tool("huge");
            huge["inputSchema"]["pad"] = json!("x".repeat(40 * 1024));
            Reply::Sse(format!(
                ": comment\n\ndata: {}\n\nevent: message\ndata: {}\n\n",
                json!({ "jsonrpc": "2.0", "method": "notifications/progress" }),
                result(seen, json!({ "tools": [beta, huge] }))
            ))
        }
        _ => Reply::Json(result(
            seen,
            json!({ "content": [{ "type": "text", "text": "hi" }] }),
        )),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    let id = store.save(save("fake", &fake.url)).unwrap()[0].id.clone();

    let tools = runtime
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"], "schema > 32 KiB is dropped");
    assert!(tools[1].read_only);
    assert_eq!(tools[1].description.chars().count(), 1024);
    assert!(tools.iter().all(|t| t.approval == Approval::Ask));

    let seen = fake.seen();
    let methods: Vec<_> = seen.iter().map(Seen::method).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/list"
        ]
    );
    assert_eq!(seen[0].body["params"]["protocolVersion"], "2025-03-26");
    assert_eq!(seen[0].session(), None);
    assert!(seen[1..].iter().all(|s| s.session() == Some("s1")));
    assert_eq!(
        seen[0].headers["accept"],
        "application/json, text/event-stream"
    );

    let mut session = runtime.session(&CancellationToken::new(), None).await;
    let prefixed: Vec<_> = session.tools().into_iter().map(|t| t.prefixed).collect();
    assert_eq!(prefixed, ["mcp_fake_alpha", "mcp_fake_beta"]);
    let out = session
        .call(
            "mcp_fake_alpha",
            json!({ "q": 1 }),
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(
        out,
        McpCallResult::Completed {
            content: vec![json!({ "type": "text", "text": "hi" })],
            structured_content: None,
            is_error: false,
        }
    );
    let call = fake.seen().pop().unwrap();
    assert_eq!(
        call.body["params"],
        json!({ "name": "alpha", "arguments": { "q": 1 } })
    );
}

/// A 404 on a request with a session id re-handshakes once and retries.
#[tokio::test]
async fn expired_session_rehandshakes_and_preserves_content() {
    let inits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&inits);
    let fake = Fake::start(move |seen| match seen.method() {
        "initialize" => {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Reply::Session(format!("s{n}"), initialized(seen, "2024-11-05"))
        }
        "notifications/initialized" => Reply::Status(202),
        "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
        _ if seen.session() == Some("s1") => Reply::Status(404),
        _ => Reply::Json(result(
            seen,
            json!({ "isError": true, "content": [
                { "type": "text", "text": "bad arg" },
                { "type": "image", "data": "...", "mimeType": "image/png" },
                { "type": "vendor-extension", "payload": { "nested": [1, true] } }
            ], "structuredContent": { "details": { "retryable": false }, "count": 3 } }),
        )),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    store.save(save("svc", &fake.url)).unwrap();
    let cancel = CancellationToken::new();
    // No cache yet: session() discovers now and keeps that connection (session s1).
    let mut session = runtime.session(&cancel, None).await;
    let out = session.call("mcp_svc_t", json!({}), &cancel).await;
    assert_eq!(
        out,
        McpCallResult::Completed {
            content: vec![
                json!({ "type": "text", "text": "bad arg" }),
                json!({ "type": "image", "data": "...", "mimeType": "image/png" }),
                json!({ "type": "vendor-extension", "payload": { "nested": [1, true] } }),
            ],
            structured_content: Some(json!({ "details": { "retryable": false }, "count": 3 })),
            is_error: true,
        }
    );
    let seen = fake.seen();
    let tail: Vec<_> = seen[3..]
        .iter()
        .map(|s| (s.method(), s.session()))
        .collect();
    assert_eq!(
        tail,
        [
            ("tools/call", Some("s1")),
            ("initialize", None),
            ("notifications/initialized", Some("s2")),
            ("tools/call", Some("s2")),
        ]
    );
    let unknown = session.call("mcp_other_x", json!({}), &cancel).await;
    assert!(matches!(unknown, McpCallResult::Failed { code, .. } if code == "unknown_tool"));
}

#[tokio::test]
async fn unsupported_protocol_version_is_rejected() {
    let fake = Fake::start(|seen| Reply::Json(initialized(seen, "1999-01-01"))).await;
    let (_dir, store, runtime) = runtime();
    let id = store.save(save("old", &fake.url)).unwrap()[0].id.clone();
    let error = runtime
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code(), KitErrorCode::Unsupported);
}

/// After the request is sent, a timeout leaves the outcome unknown.
#[tokio::test]
async fn timeout_after_send_is_unknown() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        "tools/list" => Reply::Json(result(seen, json!({ "tools": [tool("slow")] }))),
        _ => Reply::Hang,
    })
    .await;
    let (_dir, store, mut runtime) = runtime();
    runtime.timeouts.request = Duration::from_millis(300);
    store.save(save("slow", &fake.url)).unwrap();
    let cancel = CancellationToken::new();
    let mut session = runtime.session(&cancel, None).await;
    let out = session.call("mcp_slow_slow", json!({}), &cancel).await;
    assert_eq!(
        out,
        McpCallResult::Unknown {
            reason: "timeout".to_owned()
        }
    );
    assert_eq!(fake.seen().last().unwrap().method(), "tools/call");
}

/// Refusing before any request is sent is a plain error, not `unknown`.
#[tokio::test]
async fn unreachable_server_is_left_out_of_the_catalogue() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    drop(listener);
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ => Reply::Json(result(seen, json!({ "tools": [tool("ok")] }))),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    store.save(save("down", &url)).unwrap();
    store.save(save("up", &fake.url)).unwrap();
    let session = runtime.session(&CancellationToken::new(), None).await;
    let names: Vec<_> = session.tools().into_iter().map(|t| t.prefixed).collect();
    assert_eq!(names, ["mcp_up_ok"]);
}

/// docs/27 §3.2: a server on this process's own MCP Server port is never contacted.
#[tokio::test]
async fn own_server_port_is_skipped() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ => Reply::Json(result(seen, json!({ "tools": [tool("ok")] }))),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    store.save(save("own", &fake.url)).unwrap();
    let port = url::Url::parse(&fake.url).unwrap().port();
    let session = runtime.session(&CancellationToken::new(), port).await;
    assert!(session.tools().is_empty());
    assert!(fake.seen().is_empty());
    let session = runtime.session(&CancellationToken::new(), Some(1)).await;
    assert_eq!(session.tools().len(), 1);
}

#[test]
fn url_with_credentials_is_rejected_on_save() {
    let (_dir, store) = store();
    for (url, reason) in [
        ("https://mcp.example.com/sse?ak=abc", "credential_query"),
        (
            "https://mcp.example.com/?q=sk-abcdefghijklmnopqrstuvwx",
            "credential_query",
        ),
        ("https://user:pw@mcp.example.com/mcp", "userinfo"),
        ("http://mcp.example.com/mcp", "plain_http"),
    ] {
        let error = store.save(save("x", url)).unwrap_err();
        assert_eq!(error.details().reason(), Some(reason), "{url}");
    }
    assert!(store.list().unwrap().is_empty());
    assert!(
        store
            .save(save("x", "https://mcp.example.com/mcp?region=cn"))
            .is_ok()
    );
    let error = store
        .save(save("x", "https://other.example.com/"))
        .unwrap_err();
    assert_eq!(error.details().reason(), Some("duplicate_slug"));
    let error = store
        .save(save("Bad-Slug", "https://other.example.com/"))
        .unwrap_err();
    assert_eq!(error.details().field(), Some("slug"));
}

/// Credentials live only in the sealed secret and are attached per request.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn credential_query_is_sent_but_never_stored_in_url() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ => Reply::Json(result(seen, json!({ "tools": [tool("t")] }))),
    })
    .await;
    let (dir, store, runtime) = runtime();
    let url = format!("{}?region=cn", fake.url);
    let mut input = save("map", &url);
    input.secret = Some(ServerSecret {
        headers: Some(BTreeMap::from([(
            "Authorization".to_owned(),
            "Bearer hdr-123".to_owned(),
        )])),
        query: Some(BTreeMap::from([("ak".to_owned(), "qry-456".to_owned())])),
        env: None,
    });
    let listed = store.save(input).unwrap();
    assert_eq!(listed[0].transport, McpTransport::Http { url: url.clone() });
    assert_eq!(listed[0].secret_keys, ["Authorization", "ak"]);
    let id = listed[0].id.clone();

    runtime
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap();
    let seen = fake.seen();
    assert!(seen.iter().all(|s| s.target == "/mcp?region=cn&ak=qry-456"));
    assert!(
        seen.iter()
            .all(|s| s.headers["authorization"] == "Bearer hdr-123")
    );

    let raw = rusqlite::Connection::open(dir.path().join("mcp-client.db")).unwrap();
    let stored: String = raw
        .query_row("SELECT url FROM servers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, url);
    drop(raw);
    drop(runtime);
    drop(store);
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("qry-456") && !text.contains("hdr-123"));
    }

    // Absent secret keeps it; `{}` clears it.
    let store = Store::open(dir.path()).unwrap();
    let mut update = save("map", &url);
    update.id = Some(id.clone());
    assert_eq!(store.save(update).unwrap()[0].secret_keys.len(), 2);
    let mut clear = save("map", &url);
    clear.id = Some(id);
    clear.secret = Some(ServerSecret {
        headers: None,
        query: None,
        env: None,
    });
    assert!(store.save(clear).unwrap()[0].secret_keys.is_empty());
}

#[tokio::test]
async fn approvals_survive_refresh_and_untrusted_servers_stay_out() {
    let fake = Fake::start(|seen| match seen.method() {
        "initialize" => Reply::Json(initialized(seen, "2025-03-26")),
        "notifications/initialized" => Reply::Status(202),
        _ => Reply::Json(result(seen, json!({ "tools": [tool("a"), tool("b")] }))),
    })
    .await;
    let (_dir, store, runtime) = runtime();
    let id = store.save(save("one", &fake.url)).unwrap()[0].id.clone();
    let mut untrusted = save("two", &fake.url);
    untrusted.trusted = false;
    let other = store.save(untrusted).unwrap()[1].id.clone();
    let cancel = CancellationToken::new();
    runtime.refresh(&id, &cancel).await.unwrap();
    runtime.refresh(&other, &cancel).await.unwrap();
    store.set_tool_approval(&id, "a", Approval::Auto).unwrap();
    assert_eq!(
        store
            .set_tool_approval(&id, "zzz", Approval::Auto)
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    let tools = runtime.refresh(&id, &cancel).await.unwrap();
    assert_eq!(tools[0].approval, Approval::Auto);
    assert_eq!(tools[1].approval, Approval::Ask);

    let session = runtime.session(&cancel, None).await;
    let names: Vec<_> = session
        .tools()
        .into_iter()
        .map(|t| (t.prefixed, t.approval))
        .collect();
    assert_eq!(
        names,
        [
            ("mcp_one_a".to_owned(), Approval::Auto),
            ("mcp_one_b".to_owned(), Approval::Ask)
        ]
    );
    assert_eq!(store.delete(&other).unwrap().len(), 1);
}
