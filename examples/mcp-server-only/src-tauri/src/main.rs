//! 示例宿主：只启用 mcp-server，不含 chat / providers / skills。验收用法见 `docs/90-开发任务清单.md`。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use aether_mcp_server::{
    CallContext, DEFAULT_MAX_REQUEST_BYTES, ExposedTool, KitError, McpServer, ScopeSpec,
    ServerConfig, ToolHandler,
};
use aether_observability::LogConfig;
use serde_json::{Value, json};

#[expect(
    clippy::print_stderr,
    reason = "logging may not be initialised yet; this is the only way to report a startup failure"
)]
fn main() {
    if let Err(error) = run() {
        eprintln!("example-mcp-server-only failed to start: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Before `tauri::Builder`, so logs from other plugins' setup reach the file (docs/10 §8).
    let root = aether_observability::data_root("aether-mcp-server-only")?;
    let logs = aether_observability::init(&root.join("logs"), LogConfig::default(), None)?;
    let server = McpServer::new(
        &root,
        ServerConfig {
            default_port: 37651,
            token_prefix: "dkms",
        },
        vec![
            ScopeSpec {
                id: "counter.read",
                label_key: "example.scope.read",
                destructive: false,
            },
            ScopeSpec {
                id: "counter.control",
                label_key: "example.scope.control",
                destructive: true,
            },
        ],
        counter_tools(),
    )?;
    let aether = tauri_plugin_aether::Builder::new(logs)
        .mcp_server(server)
        .build()?;
    tauri::Builder::default()
        .plugin(aether)
        .setup(|app| {
            tauri::Manager::add_capability(
                app,
                r#"{ "identifier": "aether", "windows": ["main"],
                     "permissions": ["aether:logs", "aether:mcp-server-read", "aether:mcp-server-control"] }"#,
            )?;
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}

/// A shared counter: `counter_get` is read-only, `counter_reset` is control-class.
fn counter_tools() -> Vec<ExposedTool> {
    let counter = Arc::new(AtomicI64::new(0));
    let get = Arc::clone(&counter);
    let get: ToolHandler = Arc::new(move |_args: Value, _ctx: CallContext| {
        let value = get.fetch_add(1, Ordering::Relaxed) + 1;
        Box::pin(async move { Ok::<_, KitError>(json!({ "reads": value })) })
    });
    let reset: ToolHandler = Arc::new(move |_args: Value, ctx: CallContext| {
        counter.store(0, Ordering::Relaxed);
        Box::pin(async move { Ok(json!({ "reset": true, "by": ctx.token_id })) })
    });
    let no_args = json!({ "type": "object", "properties": {} });
    vec![
        ExposedTool {
            name: "counter_get",
            description: "Returns how many times the counter has been read (increments it).",
            input_schema: no_args.clone(),
            scope: "counter.read",
            read_only: true,
            destructive: false,
            idempotent: false,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            handler: get,
        },
        ExposedTool {
            name: "counter_reset",
            description: "Resets the counter to zero.",
            input_schema: no_args,
            scope: "counter.control",
            read_only: false,
            destructive: true,
            idempotent: true,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            handler: reset,
        },
    ]
}
