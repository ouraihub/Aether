//! MCP client configuration DTOs (docs/13 §4). Requests reject unknown fields.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use aether_support::Approval;

/// A discovered tool and its per-tool approval default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolDto {
    /// Tool name as reported by the server.
    pub name: String,
    /// Description, truncated to 1024 characters.
    pub description: String,
    /// `annotations.readOnlyHint`; a UI suggestion only, never trusted.
    pub read_only: bool,
    /// Default `ask`.
    pub approval: Approval,
}

/// A configured server as shown to the UI: credentials only as key names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerDto {
    /// Server id.
    pub id: String,
    /// `[a-z0-9_]{1,24}`; tool names become `mcp_<slug>_<tool>`.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Masked URL.
    pub url: String,
    /// Names of stored header/query credentials, never values.
    pub secret_keys: Vec<String>,
    /// Enabled by the user.
    pub enabled: bool,
    /// Trusted by the user.
    pub trusted: bool,
    /// The user explicitly allowed a private-network address.
    pub allow_private: bool,
    /// Last successfully discovered tools.
    pub tools: Vec<ToolDto>,
}

/// Credentials sent with every request; write-only, so no `Debug`/`Serialize`.
#[derive(Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ServerSecret {
    /// Request headers such as `Authorization`.
    #[serde(default)]
    #[ts(optional)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Query parameters such as `ak`; never stored in `url`.
    #[serde(default)]
    #[ts(optional)]
    pub query: Option<BTreeMap<String, String>>,
}

/// Create (no `id`) or update a server.
#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ServerSave {
    /// Existing server id when updating.
    #[serde(default)]
    #[ts(optional)]
    pub id: Option<String>,
    /// Slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// URL without credentials.
    pub url: String,
    /// Absent = keep; `{}` = clear.
    #[serde(default)]
    #[ts(optional)]
    pub secret: Option<ServerSecret>,
    /// Enabled.
    pub enabled: bool,
    /// Trusted.
    pub trusted: bool,
    /// Allow a private-network address.
    pub allow_private: bool,
}
