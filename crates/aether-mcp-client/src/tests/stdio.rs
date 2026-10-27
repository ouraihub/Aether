//! stdio transport against this test binary re-run as a fixture MCP server: the ignored
//! `fixture_server` test serves JSON-RPC on stdin/stdout when `AETHER_MCP_STDIO_FIXTURE`
//! names a mode, and returns at once otherwise.
#![expect(clippy::panic, reason = "a failed match is a failed test")]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use aether_support::KitErrorCode;
use serde_json::{Value, json};

use crate::protocol::MAX_MESSAGE_BYTES;
use crate::stdio::child_env;
use crate::{CancellationToken, McpCallResult, McpTransport, ServerSave, ServerSecret, Store};

const FIXTURE_ENV: &str = "AETHER_MCP_STDIO_FIXTURE";
const FIXTURE_ARGS: [&str; 5] = [
    "tests::stdio::fixture_server",
    "--exact",
    "--ignored",
    "--quiet",
    "--test-threads=1",
];
/// Configured explicitly, so it reaches the child although its name looks like a secret.
const CONFIGURED: &str = "FIXTURE_API_TOKEN";
const PAGE_TWO: [&str; 7] = ["pid", "spawn", "slow", "big", "exit", "env", "pinged"];

#[test]
#[ignore = "child-process fixture for the stdio tests"]
fn fixture_server() {
    match std::env::var(FIXTURE_ENV).as_deref() {
        Ok("normal") => serve(),
        Ok("hang") => for _ in std::io::stdin().lines() {},
        Ok("sleep") => std::thread::sleep(Duration::from_secs(60)),
        _ => {}
    }
}

fn send(out: &mut impl Write, message: &Value) {
    writeln!(out, "{message}").unwrap();
    out.flush().unwrap();
}

fn reply(id: &Value, result: Value) -> Value {
    let mut message = json!({ "jsonrpc": "2.0", "id": id });
    message["result"] = result;
    message
}

fn text(id: &Value, text: &str) -> Value {
    reply(id, json!({ "content": [{ "type": "text", "text": text }] }))
}

fn tool(name: &str) -> Value {
    json!({ "name": name, "description": "d", "inputSchema": { "type": "object" } })
}

/// initialize → two pages of tools (a notification, a server ping and a non-JSON line on
/// the way) → tools/call. 100 KiB of stderr first: an undrained pipe would block here.
fn serve() {
    std::io::stderr()
        .write_all(&vec![b'e'; 100 * 1024])
        .unwrap();
    let mut out = std::io::stdout().lock();
    // Ends whatever libtest printed and is skipped by the client as non-JSON.
    out.write_all(b"\nnot json\n").unwrap();
    let mut pinged = false;
    let mut children = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let id = message["id"].clone();
        match message["method"].as_str() {
            None => pinged |= id == "srv-1" && message["result"].is_object(),
            Some("initialize") => send(
                &mut out,
                &reply(
                    &id,
                    json!({ "protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": { "name": "fixture" } }),
                ),
            ),
            Some("tools/list") if message["params"]["cursor"].is_null() => {
                send(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }),
                );
                send(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "ping" }),
                );
                send(
                    &mut out,
                    &reply(
                        &id,
                        json!({ "tools": [tool("echo"), tool("fail")], "nextCursor": "p2" }),
                    ),
                );
            }
            Some("tools/list") => send(
                &mut out,
                &reply(&id, json!({ "tools": PAGE_TWO.map(tool) })),
            ),
            Some("tools/call") => {
                let args = &message["params"]["arguments"];
                let answer = match message["params"]["name"].as_str().unwrap() {
                    "echo" => args.to_string(),
                    "fail" => {
                        let content = json!([
                            { "type": "text", "text": "bad arg" },
                            { "type": "image", "data": "..." }
                        ]);
                        send(
                            &mut out,
                            &reply(&id, json!({ "isError": true, "content": content })),
                        );
                        continue;
                    }
                    "pid" => std::process::id().to_string(),
                    "spawn" => {
                        let child = std::process::Command::new(std::env::current_exe().unwrap())
                            .args(FIXTURE_ARGS)
                            .env(FIXTURE_ENV, "sleep")
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn()
                            .unwrap();
                        let pids = format!("{} {}", std::process::id(), child.id());
                        children.push(child);
                        pids
                    }
                    "slow" => {
                        std::thread::sleep(Duration::from_secs(30));
                        "late".to_owned()
                    }
                    "big" => {
                        // The reader gives up first; a broken pipe here is expected.
                        let _ = writeln!(out, "{}", text(&id, &"x".repeat(MAX_MESSAGE_BYTES)));
                        let _ = out.flush();
                        continue;
                    }
                    "exit" => std::process::exit(0),
                    "env" => {
                        let leaked: Vec<String> = std::env::vars()
                            .map(|(name, _)| name)
                            .filter(|name| name != CONFIGURED && secret_like(name))
                            .collect();
                        json!({ "configured": std::env::var(CONFIGURED).ok(), "leaked": leaked })
                            .to_string()
                    }
                    "pinged" => pinged.to_string(),
                    other => format!("unknown {other}"),
                };
                send(&mut out, &text(&id, &answer));
            }
            Some(_) => {}
        }
    }
}

fn secret_like(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.ends_with("_KEY")
        || name.ends_with("_TOKEN")
        || name.ends_with("_SECRET")
        || name.contains("PASSWORD")
}

fn exe() -> String {
    std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn stdio(slug: &str, mode: &str) -> ServerSave {
    ServerSave {
        id: None,
        slug: slug.to_owned(),
        name: slug.to_owned(),
        transport: McpTransport::Stdio {
            command: exe(),
            args: FIXTURE_ARGS.map(str::to_owned).to_vec(),
            cwd: None,
        },
        secret: Some(ServerSecret {
            headers: None,
            query: None,
            env: Some(BTreeMap::from([
                (FIXTURE_ENV.to_owned(), mode.to_owned()),
                (CONFIGURED.to_owned(), "configured-value".to_owned()),
            ])),
        }),
        enabled: true,
        trusted: true,
        allow_private: true,
    }
}

fn ok_text(result: McpCallResult) -> String {
    match result {
        McpCallResult::Ok { text } => text,
        other @ (McpCallResult::Error { .. } | McpCallResult::Unknown { .. }) => {
            panic!("expected ok, got {other:?}")
        }
    }
}

fn unknown(reason: &str) -> McpCallResult {
    McpCallResult::Unknown {
        reason: reason.to_owned(),
    }
}

fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
}

/// Termination runs on the runtime, so poll while yielding to it.
async fn assert_ends(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} still running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn discovers_pages_calls_and_maps_errors_over_stdio() {
    let (_dir, store) = super::store();
    let listed = store.save(stdio("local", "normal")).unwrap();
    assert_eq!(
        listed[0].transport,
        McpTransport::Stdio {
            command: exe(),
            args: FIXTURE_ARGS.map(str::to_owned).to_vec(),
            cwd: None
        }
    );
    assert_eq!(listed[0].secret_keys, [FIXTURE_ENV, CONFIGURED]);
    assert!(!listed[0].allow_private, "allow_private is http only");

    let cancel = CancellationToken::new();
    let tools = store.refresh(&listed[0].id, &cancel).await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names[..2], ["echo", "fail"]);
    assert_eq!(names[2..], PAGE_TWO);

    let mut session = store.session(&cancel, None).await;
    let echoed = ok_text(
        session
            .call("mcp_local_echo", json!({ "q": 1 }), &cancel)
            .await,
    );
    assert_eq!(echoed, r#"{"q":1}"#);
    assert_eq!(
        session.call("mcp_local_fail", json!({}), &cancel).await,
        McpCallResult::Error {
            code: "tool_error".to_owned(),
            message: "bad arg\n[unsupported content: image]".to_owned()
        }
    );
    // Cached tools: this session started its own process on the first call. Its parent
    // environment is filtered (`child_env_drops_credential_names`), configured env kept.
    let env: Value = serde_json::from_str(&ok_text(
        session.call("mcp_local_env", json!({}), &cancel).await,
    ))
    .unwrap();
    assert_eq!(env["configured"], "configured-value");
    assert_eq!(env["leaked"], json!([]));
}

/// The fixture answers a server-initiated `ping` only during discovery; the client must
/// have replied before the second page arrived.
#[tokio::test]
async fn server_ping_is_answered() {
    let (_dir, store) = super::store();
    store.save(stdio("ping", "normal")).unwrap();
    let cancel = CancellationToken::new();
    // No cache yet: discovery and the call share one process.
    let mut session = store.session(&cancel, None).await;
    assert_eq!(
        ok_text(session.call("mcp_ping_pinged", json!({}), &cancel).await),
        "true"
    );
}

/// A timed-out call is `unknown`, and the half-answered process is ended and replaced.
#[tokio::test]
async fn call_timeout_is_unknown_and_ends_the_process() {
    let (_dir, mut store) = super::store();
    store.timeouts.request = Duration::from_millis(1500);
    store.save(stdio("t", "normal")).unwrap();
    let cancel = CancellationToken::new();
    let mut session = store.session(&cancel, None).await;
    let pid: u32 = ok_text(session.call("mcp_t_pid", json!({}), &cancel).await)
        .parse()
        .unwrap();
    assert_eq!(
        session.call("mcp_t_slow", json!({}), &cancel).await,
        unknown("timeout")
    );
    assert_ends(pid).await;
    let next: u32 = ok_text(session.call("mcp_t_pid", json!({}), &cancel).await)
        .parse()
        .unwrap();
    assert_ne!(next, pid, "a fresh process serves the next call");
}

/// Cancelling a call ends the server and the processes it started.
#[tokio::test]
async fn cancel_ends_the_process_tree() {
    let (_dir, store) = super::store();
    store.save(stdio("c", "normal")).unwrap();
    let cancel = CancellationToken::new();
    let mut session = store.session(&cancel, None).await;
    let pids = ok_text(session.call("mcp_c_spawn", json!({}), &cancel).await);
    let pids: Vec<u32> = pids.split(' ').map(|pid| pid.parse().unwrap()).collect();
    assert!(alive(pids[1]));
    let call = CancellationToken::new();
    let trigger = call.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    assert_eq!(
        session.call("mcp_c_slow", json!({}), &call).await,
        unknown("cancelled")
    );
    for pid in pids {
        assert_ends(pid).await;
    }
}

/// Dropping the session (the run ended) ends its processes.
#[tokio::test]
async fn dropping_the_session_ends_the_process() {
    let (_dir, store) = super::store();
    store.save(stdio("d", "normal")).unwrap();
    let cancel = CancellationToken::new();
    let mut session = store.session(&cancel, None).await;
    let pid: u32 = ok_text(session.call("mcp_d_pid", json!({}), &cancel).await)
        .parse()
        .unwrap();
    drop(session);
    assert_ends(pid).await;
}

/// A line over 2 MiB disconnects; a server that exits mid-call is `unknown` too.
#[tokio::test]
async fn oversized_line_and_exit_disconnect() {
    let (_dir, store) = super::store();
    store.save(stdio("b", "normal")).unwrap();
    let cancel = CancellationToken::new();
    let mut session = store.session(&cancel, None).await;
    let pid: u32 = ok_text(session.call("mcp_b_pid", json!({}), &cancel).await)
        .parse()
        .unwrap();
    assert_eq!(
        session.call("mcp_b_big", json!({}), &cancel).await,
        unknown("budget_exceeded")
    );
    assert_ends(pid).await;
    assert_eq!(
        session.call("mcp_b_exit", json!({}), &cancel).await,
        unknown("network")
    );
    assert!(matches!(
        session.call("mcp_b_echo", json!({}), &cancel).await,
        McpCallResult::Ok { .. }
    ));
}

/// No answer to `initialize` within the startup deadline: the server is left out.
#[tokio::test]
async fn startup_timeout_leaves_the_server_out() {
    let (_dir, mut store) = super::store();
    store.timeouts.startup = Duration::from_millis(1500);
    let id = store.save(stdio("h", "hang")).unwrap()[0].id.clone();
    let started = Instant::now();
    let error = store
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code(), KitErrorCode::Timeout);
    assert!(started.elapsed() < Duration::from_secs(10));
    let session = store.session(&CancellationToken::new(), None).await;
    assert!(session.tools().is_empty());
}

#[tokio::test]
async fn missing_command_is_a_bad_request() {
    let (_dir, store) = super::store();
    let mut input = stdio("m", "normal");
    input.transport = McpTransport::Stdio {
        command: "aether-no-such-command-x".to_owned(),
        args: Vec::new(),
        cwd: None,
    };
    let id = store.save(input).unwrap()[0].id.clone();
    let error = store
        .refresh(&id, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.details().field(), Some("command"));
    assert_eq!(error.details().reason(), Some("not_found"));
}

#[test]
fn child_env_drops_credential_names() {
    let parent = [
        "PATH",
        "OPENAI_API_KEY",
        "GITHUB_TOKEN",
        "client_secret",
        "DB_PASSWORD_FILE",
        "KEYRING",
        "TOKENIZERS",
    ]
    .map(|name| (OsString::from(name), OsString::from("v")));
    let configured = BTreeMap::from([("GITHUB_TOKEN".to_owned(), "mine".to_owned())]);
    let env = child_env(parent, &configured);
    let names: Vec<_> = env
        .iter()
        .map(|(name, value)| format!("{}={}", name.to_string_lossy(), value.to_string_lossy()))
        .collect();
    assert_eq!(
        names,
        ["PATH=v", "KEYRING=v", "TOKENIZERS=v", "GITHUB_TOKEN=mine"]
    );
}

/// `npx`/`uvx` are `.cmd` shims on Windows; a bare name must find them via PATH + PATHEXT
/// (in PATHEXT order), and a name with a separator or extension must stay untouched.
#[cfg(windows)]
#[test]
fn bare_names_resolve_through_pathext_on_windows() {
    use crate::stdio::resolve_program;
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(second.path().join("tool.cmd"), "@echo off").unwrap();
    std::fs::write(second.path().join("tool.exe"), "").unwrap();
    let path = std::env::join_paths([first.path(), second.path()]).unwrap();
    let env = [
        (OsString::from("Path"), path),
        (OsString::from("PATHEXT"), OsString::from(".EXE;.CMD")),
    ];
    assert_eq!(
        resolve_program("tool", &env),
        second.path().join("tool.EXE").into_os_string()
    );
    let only_cmd = [
        (env[0].0.clone(), env[0].1.clone()),
        (OsString::from("PATHEXT"), OsString::from(".CMD")),
    ];
    assert_eq!(
        resolve_program("tool", &only_cmd),
        second.path().join("tool.CMD").into_os_string()
    );
    assert_eq!(resolve_program("missing", &env), OsString::from("missing"));
    assert_eq!(
        resolve_program("tool.cmd", &env),
        OsString::from("tool.cmd")
    );
    assert_eq!(resolve_program(r".\tool", &env), OsString::from(r".\tool"));
}

fn with_transport(command: &str, args: Vec<String>, cwd: Option<String>) -> ServerSave {
    let mut input = stdio("v", "normal");
    input.transport = McpTransport::Stdio {
        command: command.to_owned(),
        args,
        cwd,
    };
    input
}

#[test]
fn invalid_stdio_config_is_rejected_on_save() {
    let (dir, store) = super::store();
    let absent = dir.path().join("absent").to_string_lossy().into_owned();
    let cases = [
        (with_transport(" ", Vec::new(), None), "command", "empty"),
        (
            with_transport(&"c".repeat(1025), Vec::new(), None),
            "command",
            "too_long",
        ),
        (
            with_transport("a\nb", Vec::new(), None),
            "command",
            "invalid",
        ),
        (
            with_transport("x", vec![String::new(); 65], None),
            "args",
            "too_many",
        ),
        (
            with_transport("x", vec!["a".repeat(4097)], None),
            "args",
            "too_long",
        ),
        (
            with_transport(
                "x",
                vec!["--key".to_owned(), "sk-abcdefghijklmnopqrstuvwx".to_owned()],
                None,
            ),
            "args",
            "credential_arg",
        ),
        (
            with_transport(
                "x",
                vec!["--key=sk-abcdefghijklmnopqrstuvwx".to_owned()],
                None,
            ),
            "args",
            "credential_arg",
        ),
        (
            with_transport("x", Vec::new(), Some("rel/dir".to_owned())),
            "cwd",
            "not_absolute",
        ),
        (
            with_transport("x", Vec::new(), Some(absent)),
            "cwd",
            "not_found",
        ),
    ];
    for (input, field, reason) in cases {
        let error = store.save(input).unwrap_err();
        assert_eq!(
            (error.details().field(), error.details().reason()),
            (Some(field), Some(reason))
        );
    }
    let mut bad_env = stdio("v", "normal");
    bad_env.secret = Some(ServerSecret {
        headers: None,
        query: None,
        env: Some(BTreeMap::from([("1BAD".to_owned(), "v".to_owned())])),
    });
    assert_eq!(
        store.save(bad_env).unwrap_err().details().reason(),
        Some("env_key")
    );
    let mut headers = stdio("v", "normal");
    headers.secret = Some(ServerSecret {
        headers: Some(BTreeMap::from([("A".to_owned(), "b".to_owned())])),
        query: None,
        env: None,
    });
    assert_eq!(
        store.save(headers).unwrap_err().details().reason(),
        Some("transport_mismatch")
    );
    let mut http_env = super::save("w", "https://mcp.example.com/mcp");
    http_env.secret = stdio("w", "normal").secret;
    assert_eq!(
        store.save(http_env).unwrap_err().details().reason(),
        Some("transport_mismatch")
    );
    assert!(store.list().unwrap().is_empty());

    let cwd = dir.path().to_string_lossy().into_owned();
    let max = with_transport(&"c".repeat(1024), vec!["a".repeat(4096); 64], Some(cwd));
    assert!(store.save(max).is_ok(), "limits are inclusive");
}

/// Switching transport must replace the credentials: an http secret never reaches a process.
#[test]
fn transport_change_requires_new_credentials() {
    let (_dir, store) = super::store();
    let id = store.save(stdio("s", "normal")).unwrap()[0].id.clone();
    let mut http = super::save("s", "https://mcp.example.com/mcp");
    http.id = Some(id.clone());
    assert_eq!(
        store.save(http).unwrap_err().details().reason(),
        Some("transport_changed")
    );
    let mut http = super::save("s", "https://mcp.example.com/mcp");
    http.id = Some(id);
    http.secret = Some(ServerSecret {
        headers: None,
        query: None,
        env: None,
    });
    let listed = store.save(http).unwrap();
    assert!(listed[0].secret_keys.is_empty());
    assert!(matches!(listed[0].transport, McpTransport::Http { .. }));
}

#[test]
fn transport_json_is_tagged_and_strict() {
    let stdio: McpTransport =
        serde_json::from_value(json!({ "kind": "stdio", "command": "npx", "args": ["-y", "pkg"] }))
            .unwrap();
    assert_eq!(
        stdio,
        McpTransport::Stdio {
            command: "npx".to_owned(),
            args: vec!["-y".to_owned(), "pkg".to_owned()],
            cwd: None
        }
    );
    assert_eq!(
        serde_json::to_value(McpTransport::Http {
            url: "u".to_owned()
        })
        .unwrap(),
        json!({ "kind": "http", "url": "u" })
    );
    for bad in [
        json!({ "kind": "http", "url": "u", "command": "x" }),
        json!({ "kind": "stdio", "command": "x" }),
        json!({ "kind": "sse", "url": "u" }),
        json!({ "url": "u" }),
    ] {
        assert!(
            serde_json::from_value::<McpTransport>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

/// Migration 1 → 2 keeps http servers, their tools and approvals.
#[test]
fn migration_2_keeps_http_servers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-client.db");
    let conn = aether_support::db::open(&path, &crate::store::MIGRATIONS[..1]).unwrap();
    conn.execute_batch(
        "INSERT INTO servers (id, slug, name, url, enabled, trusted, allow_private, position,
           created_at, updated_at) VALUES ('s1', 'old', 'Old', 'https://mcp.example.com/mcp',
           1, 1, 1, 0, 1, 1);
         INSERT INTO tools (server_id, name, description, input_schema, approval, fetched_at)
           VALUES ('s1', 't', 'd', '{}', 'auto', 1);",
    )
    .unwrap();
    drop(conn);
    let store = Store::open(dir.path()).unwrap();
    let listed = store.list().unwrap();
    assert_eq!(
        listed[0].transport,
        McpTransport::Http {
            url: "https://mcp.example.com/mcp".to_owned()
        }
    );
    assert!(listed[0].allow_private && listed[0].enabled && listed[0].trusted);
    assert_eq!(listed[0].tools[0].approval, crate::Approval::Auto);
    let raw = rusqlite::Connection::open(&path).unwrap();
    let version: u32 = raw
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let error = raw
        .execute("UPDATE servers SET transport = 'sse'", [])
        .unwrap_err();
    assert!(error.to_string().contains("CHECK"), "{error}");
}
