//! Tauri 接入层：按 feature 注册命令与权限，模块间组合只发生在这里。
//!
//! 设计：`docs/15-详细设计-接入层.md`。

use std::sync::Arc;

use aether_observability::LogHandle;
use tauri::plugin::TauriPlugin;
use tauri::{Manager, Runtime};

#[cfg(feature = "chat")]
mod chat;
mod host;
mod logs;
#[cfg(feature = "mcp-client")]
mod mcp_client;
#[cfg(feature = "mcp-server")]
mod mcp_server;
#[cfg(feature = "providers")]
mod providers;
#[cfg(feature = "search")]
mod search;
#[cfg(feature = "skills")]
mod skills;
#[cfg(test)]
mod tests;
#[cfg(feature = "chat")]
mod tools;

#[cfg(feature = "chat")]
pub use aether_chat::{Budget, ConversationStore};
#[cfg(feature = "chat")]
pub use chat::ChatConfig;
pub use host::*;

#[cfg(feature = "chat")]
type ChatParts = (ChatConfig, Option<Arc<dyn ConversationStore>>);

/// Assembles the plugin from host-created instances (docs/15 §2).
pub struct Builder {
    host: Arc<dyn KitHost>,
    logs: LogHandle,
    #[cfg(feature = "chat")]
    chat: Option<ChatParts>,
    #[cfg(feature = "mcp-client")]
    mcp_client: Option<aether_mcp_client::Store>,
    #[cfg(feature = "mcp-server")]
    mcp_server: Option<aether_mcp_server::McpServer>,
    #[cfg(feature = "providers")]
    providers: Option<aether_providers::Store>,
    #[cfg(feature = "skills")]
    skills: Option<aether_skills::Store>,
    #[cfg(feature = "search")]
    search: Option<aether_search::Store>,
}

impl Builder {
    /// Starts a builder; `logs` comes from `aether_observability::init`.
    #[must_use]
    pub fn new(host: Arc<dyn KitHost>, logs: LogHandle) -> Self {
        Self {
            host,
            logs,
            #[cfg(feature = "chat")]
            chat: None,
            #[cfg(feature = "mcp-client")]
            mcp_client: None,
            #[cfg(feature = "mcp-server")]
            mcp_server: None,
            #[cfg(feature = "providers")]
            providers: None,
            #[cfg(feature = "skills")]
            skills: None,
            #[cfg(feature = "search")]
            search: None,
        }
    }

    /// Chat runs (feature `chat`, needs `providers`). `store: None` runs without history:
    /// no conversation id, and the `aether:chat-history` commands are not registered.
    #[cfg(feature = "chat")]
    #[must_use]
    pub fn chat(mut self, config: ChatConfig, store: Option<Arc<dyn ConversationStore>>) -> Self {
        self.chat = Some((config, store));
        self
    }

    /// MCP client store behind the `mcp_client_*` commands; with `chat`, its enabled and
    /// trusted servers' tools join every run's catalogue (feature `mcp-client`).
    #[cfg(feature = "mcp-client")]
    #[must_use]
    pub fn mcp_client(mut self, store: aether_mcp_client::Store) -> Self {
        self.mcp_client = Some(store);
        self
    }

    /// Registers the external MCP server (feature `mcp-server`).
    #[cfg(feature = "mcp-server")]
    #[must_use]
    pub fn mcp_server(mut self, server: aether_mcp_server::McpServer) -> Self {
        self.mcp_server = Some(server);
        self
    }

    /// Provider store behind the `providers_*` commands (feature `providers`).
    #[cfg(feature = "providers")]
    #[must_use]
    pub fn providers(mut self, store: aether_providers::Store) -> Self {
        self.providers = Some(store);
        self
    }

    /// Skill store behind the `skills_*` commands and run snapshots (feature `skills`).
    #[cfg(feature = "skills")]
    #[must_use]
    pub fn skills(mut self, store: aether_skills::Store) -> Self {
        self.skills = Some(store);
        self
    }
    /// Search settings and execution (feature `search`).
    #[cfg(feature = "search")]
    #[must_use]
    pub fn search(mut self, store: aether_search::Store) -> Self {
        self.search = Some(store);
        self
    }

    /// Builds the plugin. Only commands of enabled features are registered.
    ///
    /// # Errors
    /// `bad_request{missing_instance}` when an enabled feature got no instance (docs/15 §2);
    /// `internal` when the chat HTTP client cannot be built.
    pub fn build<R: Runtime>(self) -> Result<TauriPlugin<R>, KitError> {
        let Self {
            host,
            logs,
            #[cfg(feature = "chat")]
            chat,
            #[cfg(feature = "mcp-client")]
            mcp_client,
            #[cfg(feature = "mcp-server")]
            mcp_server,
            #[cfg(feature = "providers")]
            providers,
            #[cfg(feature = "skills")]
            skills,
            #[cfg(feature = "search")]
            search,
        } = self;
        #[cfg(feature = "providers")]
        let providers = providers::Providers::new(
            providers.ok_or_else(|| KitError::bad_request("providers", "missing_instance"))?,
        );
        #[cfg(feature = "search")]
        let search = search.map(Arc::new);
        #[cfg(feature = "chat")]
        let chat = {
            let (config, store) =
                chat.ok_or_else(|| KitError::bad_request("chat", "missing_instance"))?;
            #[cfg(feature = "search")]
            let result = chat::Chat::new(
                config,
                store,
                search
                    .as_ref()
                    .ok_or_else(|| KitError::bad_request("search", "missing_instance"))?
                    .clone(),
            )?;
            #[cfg(not(feature = "search"))]
            let result = chat::Chat::new(config, store)?;
            result
        };
        #[cfg(feature = "mcp-client")]
        let mcp_client = mcp_client::McpClient {
            store: Arc::new(
                mcp_client
                    .ok_or_else(|| KitError::bad_request("mcp-client", "missing_instance"))?,
            ),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        #[cfg(feature = "mcp-server")]
        let mcp_server =
            mcp_server.ok_or_else(|| KitError::bad_request("mcp-server", "missing_instance"))?;
        #[cfg(feature = "skills")]
        let skills = skills::Skills {
            store: Arc::new(
                skills.ok_or_else(|| KitError::bad_request("skills", "missing_instance"))?,
            ),
        };
        #[cfg(feature = "search")]
        let search = search::Search::from_arc(
            search.ok_or_else(|| KitError::bad_request("search", "missing_instance"))?,
        );
        #[cfg(feature = "chat")]
        let has_history = chat.has_history();
        let handler: Box<dyn Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync> =
            Box::new(tauri::generate_handler![
                logs::logs_tail,
                logs::logs_get_level,
                logs::logs_set_level,
                logs::logs_ingest,
                logs::logs_open_dir,
                #[cfg(feature = "chat")]
                chat::chat_run,
                #[cfg(feature = "chat")]
                chat::chat_cancel,
                #[cfg(feature = "chat")]
                tools::chat_approve,
                #[cfg(feature = "chat")]
                chat::chat_conversations,
                #[cfg(feature = "chat")]
                chat::chat_conversation,
                #[cfg(feature = "chat")]
                chat::chat_delete,
                #[cfg(feature = "chat")]
                chat::chat_rename,
                #[cfg(feature = "chat")]
                chat::chat_edit_turn,
                #[cfg(feature = "chat")]
                chat::chat_remove_turn,
                #[cfg(feature = "chat")]
                chat::chat_fork,
                #[cfg(feature = "chat")]
                chat::chat_select_turn,
                #[cfg(feature = "mcp-client")]
                mcp_client::mcp_client_list,
                #[cfg(feature = "mcp-client")]
                mcp_client::mcp_client_save,
                #[cfg(feature = "mcp-client")]
                mcp_client::mcp_client_delete,
                #[cfg(feature = "mcp-client")]
                mcp_client::mcp_client_refresh,
                #[cfg(feature = "mcp-client")]
                mcp_client::mcp_client_set_tool_approval,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_status,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_tokens,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_scopes,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_start,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_stop,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_set_port,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_issue_token,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_revoke_token,
                #[cfg(feature = "mcp-server")]
                mcp_server::mcp_server_set_scopes,
                #[cfg(feature = "providers")]
                providers::providers_list,
                #[cfg(feature = "providers")]
                providers::providers_save,
                #[cfg(feature = "providers")]
                providers::providers_delete,
                #[cfg(feature = "providers")]
                providers::providers_model_options,
                #[cfg(feature = "providers")]
                providers::providers_probe,
                #[cfg(feature = "providers")]
                providers::providers_cancel,
                #[cfg(feature = "skills")]
                skills::skills_list,
                #[cfg(feature = "skills")]
                skills::skills_get,
                #[cfg(feature = "skills")]
                skills::skills_preview,
                #[cfg(feature = "skills")]
                skills::skills_install,
                #[cfg(feature = "skills")]
                skills::skills_save,
                #[cfg(feature = "skills")]
                skills::skills_duplicate,
                #[cfg(feature = "skills")]
                skills::skills_reorder,
                #[cfg(feature = "skills")]
                skills::skills_delete,
                #[cfg(feature = "search")]
                search::search_settings,
                #[cfg(feature = "search")]
                search::search_save,
                #[cfg(feature = "search")]
                search::search_save_preferences,
                #[cfg(feature = "search")]
                search::search_save_key,
                #[cfg(feature = "search")]
                search::search_remove_key,
                #[cfg(feature = "search")]
                search::search_set_default,
                #[cfg(feature = "search")]
                search::search_reveal,
                #[cfg(feature = "search")]
                search::search_check,
                #[cfg(feature = "search")]
                search::search_query,
                #[cfg(feature = "search")]
                search::search_fetch,
                #[cfg(feature = "search")]
                search::search_cancel,
            ]);
        Ok(tauri::plugin::Builder::<R>::new("aether")
            .invoke_handler(move |invoke| {
                // Without a store the history commands do not exist (docs/15 §5): `false`
                // makes Tauri answer "Command not found", as for a disabled feature.
                #[cfg(feature = "chat")]
                if !has_history && chat::HISTORY_COMMANDS.contains(&invoke.message.command()) {
                    return false;
                }
                handler(invoke)
            })
            .setup(move |app, _api| {
                // Lands in the file only when the host called aether_observability::init first (docs/10 §8).
                tracing::info!("aether.plugin.setup");
                app.manage(host);
                app.manage(logs);
                #[cfg(feature = "chat")]
                {
                    chat.spawn_image_purge();
                    app.manage(chat);
                }
                #[cfg(feature = "mcp-client")]
                app.manage(mcp_client);
                #[cfg(feature = "mcp-server")]
                app.manage(mcp_server);
                #[cfg(feature = "providers")]
                app.manage(providers);
                #[cfg(feature = "skills")]
                app.manage(skills);
                #[cfg(feature = "search")]
                app.manage(search);
                Ok(())
            })
            .on_event(on_event)
            .build())
    }
}

/// Lifecycle (docs/15 §6). `Exit` rather than `ExitRequested`, which the host may prevent.
fn on_event<R: Runtime>(app: &tauri::AppHandle<R>, event: &tauri::RunEvent) {
    #[cfg(feature = "chat")]
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = event
    {
        app.state::<chat::Chat>().cancel_window(label);
    }
    if matches!(event, tauri::RunEvent::Exit) {
        #[cfg(feature = "chat")]
        app.state::<chat::Chat>().shutdown();
        #[cfg(feature = "mcp-client")]
        app.state::<mcp_client::McpClient>().cancel.cancel();
        #[cfg(feature = "mcp-server")]
        if let Err(error) =
            tauri::async_runtime::block_on(app.state::<aether_mcp_server::McpServer>().stop())
        {
            tracing::warn!(code = ?error.code(), "mcp_server.stop_failed");
        }
        if let Err(error) = app.state::<LogHandle>().flush() {
            tracing::warn!(%error, "logs.flush_failed");
        }
    }
}
