//! `mcp_server_*` commands (docs/15 §5): thin wrappers over the host's `McpServer`.
//! Read commands are in `aether:mcp-server-read`, the rest in `aether:mcp-server-control`.
#![allow(
    clippy::unreachable,
    reason = "tauri's #[command] expansion for async commands borrowing State uses unreachable!"
)]

use aether_mcp_server::{IssuedToken, McpServer, ScopeState, Status, TokenRecord};
use aether_support::KitError;
use tauri::State;

/// Longest accepted token name / scope list.
const MAX_NAME: usize = 200;
const MAX_SCOPES: usize = 100;

/// Records a failed command as `aether.command.failed` (docs/15 §5).
fn logged<T>(command: &'static str, result: Result<T, KitError>) -> Result<T, KitError> {
    if let Err(error) = &result {
        tracing::warn!(command, code = ?error.code(), "aether.command.failed");
    }
    result
}

fn check_scopes(scopes: &[String]) -> Result<Vec<&str>, KitError> {
    if scopes.len() > MAX_SCOPES {
        return Err(KitError::bad_request("scopes", "too_many"));
    }
    Ok(scopes.iter().map(String::as_str).collect())
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "tauri injects State by value"
)]
pub(crate) fn mcp_server_status(server: State<'_, McpServer>) -> Status {
    server.status()
}

#[tauri::command]
pub(crate) async fn mcp_server_tokens(
    server: State<'_, McpServer>,
) -> Result<Vec<TokenRecord>, KitError> {
    logged("mcp_server_tokens", server.tokens())
}

#[tauri::command]
pub(crate) async fn mcp_server_scopes(
    server: State<'_, McpServer>,
) -> Result<Vec<ScopeState>, KitError> {
    logged("mcp_server_scopes", server.scopes())
}

#[tauri::command]
pub(crate) async fn mcp_server_start(server: State<'_, McpServer>) -> Result<Status, KitError> {
    logged("mcp_server_start", server.start().await)
}

#[tauri::command]
pub(crate) async fn mcp_server_stop(server: State<'_, McpServer>) -> Result<Status, KitError> {
    logged("mcp_server_stop", server.stop().await)
}

#[tauri::command]
pub(crate) async fn mcp_server_set_port(
    server: State<'_, McpServer>,
    port: u16,
) -> Result<Status, KitError> {
    logged("mcp_server_set_port", server.set_port(port).await)
}

#[tauri::command]
pub(crate) async fn mcp_server_issue_token(
    server: State<'_, McpServer>,
    name: String,
    scopes: Vec<String>,
) -> Result<IssuedToken, KitError> {
    let result = if name.chars().count() > MAX_NAME {
        Err(KitError::bad_request("name", "too_long"))
    } else {
        check_scopes(&scopes).and_then(|scopes| server.issue_token(&name, &scopes))
    };
    logged("mcp_server_issue_token", result)
}

#[tauri::command]
pub(crate) async fn mcp_server_revoke_token(
    server: State<'_, McpServer>,
    id: String,
) -> Result<(), KitError> {
    logged("mcp_server_revoke_token", server.revoke_token(&id))
}

#[tauri::command]
pub(crate) async fn mcp_server_set_scopes(
    server: State<'_, McpServer>,
    ids: Vec<String>,
) -> Result<Vec<ScopeState>, KitError> {
    let result = check_scopes(&ids)
        .and_then(|ids| server.set_enabled_scopes(&ids))
        .and_then(|()| server.scopes());
    logged("mcp_server_set_scopes", result)
}
