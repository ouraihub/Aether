//! Tauri commands, ACL and lifecycle adapters for host-created application services.

#[cfg(any(
    feature = "chat",
    feature = "conversations",
    feature = "providers",
    feature = "skills",
    feature = "search",
    feature = "mcp-client"
))]
use std::sync::Arc;

use aether_observability::LogHandle;
use aether_support::KitError;
use tauri::plugin::TauriPlugin;
use tauri::{Manager, Runtime};

#[cfg(feature = "conversations")]
mod conversations;
mod logs;
#[cfg(feature = "mcp-client")]
mod mcp_client;
#[cfg(feature = "mcp-server")]
mod mcp_server;
#[cfg(feature = "providers")]
mod providers;
#[cfg(feature = "chat")]
mod runs;
#[cfg(feature = "search")]
mod search;
#[cfg(feature = "skills")]
mod skills;
#[cfg(test)]
mod tests;

/// The host owns service construction and shares the same Arc with other consumers.
pub struct Builder {
    logs: LogHandle,
    #[cfg(feature = "chat")]
    runs: Option<Arc<aether_runtime::RunService>>,
    #[cfg(feature = "conversations")]
    conversations: Option<Arc<aether_runtime::conversations::ConversationService>>,
    #[cfg(feature = "mcp-client")]
    mcp_client: Option<Arc<aether_mcp_client::McpRuntime>>,
    #[cfg(feature = "mcp-server")]
    mcp_server: Option<aether_mcp_server::McpServer>,
    #[cfg(feature = "providers")]
    providers: Option<Arc<aether_providers::Store>>,
    #[cfg(feature = "skills")]
    skills: Option<Arc<aether_skills::Store>>,
    #[cfg(feature = "search")]
    search: Option<Arc<aether_search::Store>>,
}

impl Builder {
    /// Creates the transport adapter with an existing shared diagnostic log handle.
    #[must_use]
    pub fn new(logs: LogHandle) -> Self {
        Self {
            logs,
            #[cfg(feature = "chat")]
            runs: None,
            #[cfg(feature = "conversations")]
            conversations: None,
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

    /// Injects the already-constructed run service; the plugin does not assemble runs.
    #[cfg(feature = "chat")]
    #[must_use]
    pub fn runs(mut self, runs: Arc<aether_runtime::RunService>) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Enables history commands through the independent shared conversation service.
    #[cfg(feature = "conversations")]
    #[must_use]
    pub fn conversations(
        mut self,
        conversations: Arc<aether_runtime::conversations::ConversationService>,
    ) -> Self {
        self.conversations = Some(conversations);
        self
    }

    /// Shares an MCP runtime with management commands and ordered application shutdown.
    #[cfg(feature = "mcp-client")]
    #[must_use]
    pub fn mcp_client(mut self, runtime: Arc<aether_mcp_client::McpRuntime>) -> Self {
        self.mcp_client = Some(runtime);
        self
    }

    /// Injects the independently constructed local MCP server.
    #[cfg(feature = "mcp-server")]
    #[must_use]
    pub fn mcp_server(mut self, server: aether_mcp_server::McpServer) -> Self {
        self.mcp_server = Some(server);
        self
    }

    /// Shares provider configuration storage with the host's other Native services.
    #[cfg(feature = "providers")]
    #[must_use]
    pub fn providers(mut self, store: Arc<aether_providers::Store>) -> Self {
        self.providers = Some(store);
        self
    }

    /// Shares the installed-skill store with the skill management commands.
    #[cfg(feature = "skills")]
    #[must_use]
    pub fn skills(mut self, store: Arc<aether_skills::Store>) -> Self {
        self.skills = Some(store);
        self
    }

    /// Shares search configuration storage with the search management commands.
    #[cfg(feature = "search")]
    #[must_use]
    pub fn search(mut self, store: Arc<aether_search::Store>) -> Self {
        self.search = Some(store);
        self
    }

    /// Registers the enabled IPC adapters and their shared service instances.
    ///
    /// # Errors
    /// Returns `bad_request/missing_instance` for each enabled but absent service.
    pub fn build<R: Runtime>(self) -> Result<TauriPlugin<R>, KitError> {
        let Self {
            logs,
            #[cfg(feature = "chat")]
            runs,
            #[cfg(feature = "conversations")]
            conversations,
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
        #[cfg(feature = "chat")]
        let runs =
            runs::Runs::new(runs.ok_or_else(|| KitError::bad_request("runs", "missing_instance"))?);
        #[cfg(feature = "conversations")]
        let conversations = conversations
            .ok_or_else(|| KitError::bad_request("conversations", "missing_instance"))?;
        #[cfg(feature = "providers")]
        let providers = providers::Providers::new(
            providers.ok_or_else(|| KitError::bad_request("providers", "missing_instance"))?,
        );
        #[cfg(feature = "mcp-client")]
        let mcp_client = mcp_client::McpClient {
            runtime: mcp_client
                .ok_or_else(|| KitError::bad_request("mcp-client", "missing_instance"))?,
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        #[cfg(feature = "mcp-server")]
        let mcp_server =
            mcp_server.ok_or_else(|| KitError::bad_request("mcp-server", "missing_instance"))?;
        #[cfg(feature = "skills")]
        let skills = skills::Skills {
            store: skills.ok_or_else(|| KitError::bad_request("skills", "missing_instance"))?,
        };
        #[cfg(feature = "search")]
        let search = search::Search::from_arc(
            search.ok_or_else(|| KitError::bad_request("search", "missing_instance"))?,
        );
        let handler: Box<dyn Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync> =
            Box::new(tauri::generate_handler![
                logs::logs_tail,
                logs::logs_get_level,
                logs::logs_set_level,
                logs::logs_ingest,
                logs::logs_open_dir,
                #[cfg(feature = "chat")]
                runs::chat_run,
                #[cfg(feature = "chat")]
                runs::chat_cancel,
                #[cfg(feature = "chat")]
                runs::chat_approve,
                #[cfg(feature = "chat")]
                runs::chat_snapshot,
                #[cfg(feature = "conversations")]
                conversations::chat_conversations,
                #[cfg(feature = "conversations")]
                conversations::chat_conversation,
                #[cfg(feature = "conversations")]
                conversations::chat_delete,
                #[cfg(feature = "conversations")]
                conversations::chat_rename,
                #[cfg(feature = "conversations")]
                conversations::chat_edit_turn,
                #[cfg(feature = "conversations")]
                conversations::chat_remove_turn,
                #[cfg(feature = "conversations")]
                conversations::chat_fork,
                #[cfg(feature = "conversations")]
                conversations::chat_select_turn,
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
                providers::providers_set_enabled,
                #[cfg(feature = "providers")]
                providers::providers_model_options,
                #[cfg(feature = "providers")]
                providers::providers_probe,
                #[cfg(feature = "providers")]
                providers::providers_key_save,
                #[cfg(feature = "providers")]
                providers::providers_key_delete,
                #[cfg(feature = "providers")]
                providers::providers_key_reveal,
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
            .invoke_handler(handler)
            .setup(move |app, _api| {
                // Lands in the file only when the host called aether_observability::init first (docs/10 §8).
                tracing::info!("aether.plugin.setup");
                app.manage(logs);
                #[cfg(feature = "chat")]
                {
                    for label in app.webview_windows().keys() {
                        runs.register(label);
                    }
                    app.manage(runs);
                }
                #[cfg(feature = "conversations")]
                app.manage(conversations);
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
            .on_window_ready(|window| {
                #[cfg(feature = "chat")]
                window.state::<runs::Runs>().register(window.label());
                let _ = window;
            })
            .on_event(on_event)
            .build())
    }
}

/// Exit, unlike ExitRequested, cannot be vetoed by the host.
fn on_event<R: Runtime>(app: &tauri::AppHandle<R>, event: &tauri::RunEvent) {
    #[cfg(feature = "chat")]
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = event
    {
        app.state::<runs::Runs>().close(label);
        tracing::info!("aether.plugin.window_destroyed.cancelled");
    }
    if matches!(event, tauri::RunEvent::Exit) {
        tracing::info!("aether.plugin.shutdown.started");
        // The event-loop thread waits while the executor asynchronously drains services;
        // no executor thread spins or joins another runtime.
        tauri::async_runtime::block_on(async {
            #[cfg(feature = "chat")]
            app.state::<runs::Runs>()
                .service
                .shutdown(std::time::Duration::from_secs(3))
                .await;
            #[cfg(feature = "mcp-client")]
            {
                let client = app.state::<mcp_client::McpClient>();
                client.cancel.cancel();
                client.runtime.shutdown().await;
                tracing::info!("aether.plugin.mcp_client.shutdown.complete");
            }
            #[cfg(feature = "mcp-server")]
            if let Err(error) = app.state::<aether_mcp_server::McpServer>().stop().await {
                tracing::warn!(code = ?error.code(), "mcp_server.stop_failed");
            }
            #[cfg(feature = "providers")]
            app.state::<providers::Providers>().cancel_all();
            #[cfg(feature = "search")]
            app.state::<search::Search>().cancel_all();
            tracing::info!("aether.plugin.shutdown.finished");
            if let Err(error) = app.state::<LogHandle>().flush() {
                tracing::warn!(%error, "logs.flush_failed");
            }
        });
    }
}
