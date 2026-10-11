//! MCP server status and token DTOs (docs/13 §7, §8).

use serde::Serialize;
use ts_rs::TS;

/// Current server state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Status {
    /// Listening.
    pub running: bool,
    /// Configured port.
    pub port: u16,
    /// `http://127.0.0.1:<port>/mcp` style URL clients connect to.
    pub url: String,
    /// Number of registered tools.
    #[ts(type = "number")]
    pub tool_count: usize,
}

/// A stored token; the secret itself is never kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TokenRecord {
    /// Token id.
    pub id: String,
    /// User-given name.
    pub name: String,
    /// Granted scope ids.
    pub scopes: Vec<String>,
    /// Unix milliseconds.
    #[ts(type = "number")]
    pub created_at: i64,
    /// Unix milliseconds, updated at most once a minute.
    #[ts(type = "number | null")]
    pub last_used_at: Option<i64>,
    /// Unix milliseconds once revoked.
    #[ts(type = "number | null")]
    pub revoked_at: Option<i64>,
}

/// A freshly issued token: the only response that ever carries the secret, so no `Debug`.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IssuedToken {
    /// The stored record.
    #[serde(flatten)]
    #[ts(flatten)]
    pub record: TokenRecord,
    /// `<prefix>_<base64url>`; shown once.
    pub token: String,
}

/// A declared scope and whether the user's master switch has it on (`mcp_server_scopes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScopeState {
    /// Scope id.
    pub id: String,
    /// i18n key of the label.
    pub label_key: String,
    /// Needs a second confirmation before enabling.
    pub destructive: bool,
    /// In `enabled_scopes`.
    pub enabled: bool,
}
