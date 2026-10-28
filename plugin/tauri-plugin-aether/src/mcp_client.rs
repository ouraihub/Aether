//! MCP configuration commands and refresh transport; lifecycle belongs to McpRuntime.

use std::sync::Arc;

use aether_mcp_client::{Approval, McpRuntime, ServerDto, ServerSave, Store, ToolDto};
use aether_support::KitError;
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

/// One shared runtime, including its configuration store, and adapter request cancellation.
pub(crate) struct McpClient {
    pub(crate) runtime: Arc<McpRuntime>,
    pub(crate) cancel: CancellationToken,
}

/// Runs a blocking store call off the async runtime.
async fn blocking<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&Store) -> Result<T, KitError> + Send + 'static,
) -> Result<T, KitError> {
    let store = Arc::clone(app.state::<McpClient>().runtime.store());
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
    client.runtime.refresh(&server_id, &client.cancel).await
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
