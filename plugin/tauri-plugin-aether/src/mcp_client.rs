//! `mcp_client_*` commands (docs/15 §5, docs/13 §4). The run's MCP tools are composed in
//! `tools.rs`.

use std::sync::Arc;

use aether_mcp_client::{Approval, ServerDto, ServerSave, Store, ToolDto};
use aether_support::KitError;
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

/// The store, managed as Tauri state; `chat_run` opens a session from it. `cancel` fires on
/// exit and stops in-flight "test connection" requests (docs/15 §6).
pub(crate) struct McpClient {
    pub(crate) store: Arc<Store>,
    pub(crate) cancel: CancellationToken,
}

/// Runs a blocking store call off the async runtime.
async fn blocking<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&Store) -> Result<T, KitError> + Send + 'static,
) -> Result<T, KitError> {
    let store = Arc::clone(&app.state::<McpClient>().store);
    tauri::async_runtime::spawn_blocking(move || f(&store))
        .await
        .map_err(KitError::internal)?
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "mcp_client_list"), err(level = "warn"))]
pub(crate) async fn mcp_client_list<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<ServerDto>, KitError> {
    blocking(&app, Store::list).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "mcp_client_save"), err(level = "warn"))]
pub(crate) async fn mcp_client_save<R: Runtime>(
    app: AppHandle<R>,
    input: ServerSave,
) -> Result<Vec<ServerDto>, KitError> {
    blocking(&app, move |store| store.save(input)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "mcp_client_delete"), err(level = "warn"))]
pub(crate) async fn mcp_client_delete<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<Vec<ServerDto>, KitError> {
    blocking(&app, move |store| store.delete(&id)).await
}

/// "Test connection": handshake and re-discover the server's tools.
#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "mcp_client_refresh"), err(level = "warn"))]
pub(crate) async fn mcp_client_refresh<R: Runtime>(
    app: AppHandle<R>,
    server_id: String,
) -> Result<Vec<ToolDto>, KitError> {
    let client = app.state::<McpClient>();
    client.store.refresh(&server_id, &client.cancel).await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "mcp_client_set_tool_approval"),
    err(level = "warn")
)]
pub(crate) async fn mcp_client_set_tool_approval<R: Runtime>(
    app: AppHandle<R>,
    server_id: String,
    tool: String,
    approval: Approval,
) -> Result<(), KitError> {
    blocking(&app, move |store| {
        store.set_tool_approval(&server_id, &tool, approval)
    })
    .await
}
