//! 示例宿主：providers + chat + skills + mcp-client。验收用法见 `docs/90-开发任务清单.md`。
//! 宿主工具（feature `aether`）：`read_note`（只读，auto）与 `write_note`（写入，ask），见 `notes.rs`。
//! `AETHER_NO_HISTORY=1` 启动时不传会话存储：无历史模式，`aether:chat-history` 不授权。
//! feature `skills`：技能存储与 `aether:skills`，界面经 `?skills=1` 得知（见 ui/src/App.svelte）。
//! feature `mcp-client`：外部 MCP 服务存储与 `aether:mcp-client`，界面经 `?mcp=1` 得知。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use aether_observability::LogConfig;

#[cfg(feature = "aether")]
mod notes;
#[cfg(feature = "aether")]
use notes::AssistantHost;

/// Without aether features the host has nothing to provide.
#[cfg(not(feature = "aether"))]
struct AssistantHost;
#[cfg(not(feature = "aether"))]
impl tauri_plugin_aether::KitHost for AssistantHost {}

#[expect(
    clippy::print_stderr,
    reason = "logging may not be initialised yet; this is the only way to report a startup failure"
)]
fn main() {
    if let Err(error) = run() {
        eprintln!("example-assistant failed to start: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Before `tauri::Builder`, so logs from other plugins' setup reach the file (docs/10 §8).
    let root = aether_observability::data_root("aether-assistant")?;
    let logs = aether_observability::init(&root.join("logs"), LogConfig::default(), None)?;
    #[cfg(feature = "aether")]
    let host = AssistantHost::default();
    #[cfg(not(feature = "aether"))]
    let host = AssistantHost;
    let aether = tauri_plugin_aether::Builder::new(Arc::new(host), logs);
    #[cfg(feature = "aether")]
    let history = std::env::var_os("AETHER_NO_HISTORY").is_none();
    #[cfg(feature = "aether")]
    let aether = {
        let store: Option<Arc<dyn tauri_plugin_aether::ConversationStore>> = if history {
            Some(Arc::new(aether_chat::SqliteStore::open(&root)?))
        } else {
            None
        };
        aether
            .providers(aether_providers::Store::open(&root)?)
            .chat(tauri_plugin_aether::ChatConfig::default(), store)
    };
    #[cfg(feature = "skills")]
    let aether = aether.skills(aether_skills::Store::open(&root, &[])?);
    #[cfg(feature = "mcp-client")]
    let aether = aether.mcp_client(aether_mcp_client::Store::open(&root)?);
    tauri::Builder::default()
        .plugin(aether.build()?)
        .setup(move |app| {
            // A static capability cannot name feature sets that may be compiled out, so they
            // are granted here only for builds that have them (`aether:logs` is in default.json).
            #[cfg(feature = "aether")]
            tauri::Manager::add_capability(
                app,
                if history {
                    r#"{ "identifier": "aether", "windows": ["main"], "permissions": ["aether:chat", "aether:chat-history", "aether:providers"] }"#
                } else {
                    r#"{ "identifier": "aether", "windows": ["main"], "permissions": ["aether:chat", "aether:providers"] }"#
                },
            )?;
            #[cfg(feature = "skills")]
            tauri::Manager::add_capability(
                app,
                r#"{ "identifier": "aether-skills", "windows": ["main"], "permissions": ["aether:skills"] }"#,
            )?;
            #[cfg(feature = "mcp-client")]
            tauri::Manager::add_capability(
                app,
                r#"{ "identifier": "aether-mcp-client", "windows": ["main"], "permissions": ["aether:mcp-client"] }"#,
            )?;
            // Tell the UI which optional ports exist (ui/src/App.svelte reads the query).
            #[cfg(feature = "aether")]
            {
                let query = [
                    (!history, "history=0"),
                    (cfg!(feature = "skills"), "skills=1"),
                    (cfg!(feature = "mcp-client"), "mcp=1"),
                ]
                    .into_iter()
                    .filter_map(|(on, param)| on.then_some(param))
                    .collect::<Vec<_>>()
                    .join("&");
                if !query.is_empty()
                    && let Some(window) = tauri::Manager::get_webview_window(app, "main")
                {
                    window.eval(format!("if (!location.search) location.search = '?{query}'"))?;
                }
            }
            let _ = app;
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
