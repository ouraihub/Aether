//! MCP client configuration DTOs (docs/27 §2). Requests reject unknown fields.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
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

/// How the server is reached (docs/27 §2). Requests reject unknown fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum McpTransport {
    /// Streamable HTTP.
    Http {
        /// URL without credentials; masked when listed.
        url: String,
    },
    /// A local command speaking newline-delimited JSON-RPC on stdin/stdout.
    Stdio {
        /// Program to start (a path or a name looked up in `PATH`).
        command: String,
        /// Arguments, passed as is (no shell).
        args: Vec<String>,
        /// Absolute working directory; absent = inherited.
        #[serde(default)]
        #[ts(optional)]
        cwd: Option<String>,
    },
}

impl<'de> Deserialize<'de> for McpTransport {
    /// Through a mirror enum: ts-rs rejects `deny_unknown_fields` on enums it exports.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Strict {
            Http {
                url: String,
            },
            Stdio {
                command: String,
                args: Vec<String>,
                #[serde(default)]
                cwd: Option<String>,
            },
        }
        Ok(match Strict::deserialize(deserializer)? {
            Strict::Http { url } => Self::Http { url },
            Strict::Stdio { command, args, cwd } => Self::Stdio { command, args, cwd },
        })
    }
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
    /// Transport; an http URL is masked.
    pub transport: McpTransport,
    /// Names of stored header/query/env credentials, never values.
    pub secret_keys: Vec<String>,
    /// Enabled by the user.
    pub enabled: bool,
    /// Trusted by the user.
    pub trusted: bool,
    /// The user explicitly allowed a private-network address (http only; always false for stdio).
    pub allow_private: bool,
    /// Last successfully discovered tools.
    pub tools: Vec<ToolDto>,
}

/// Credentials, write-only, so no `Debug`/`Serialize`. `headers`/`query` belong to
/// http servers, `env` to stdio servers; the other kind is rejected.
#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Environment variables for a stdio server, such as `GITHUB_TOKEN`.
    #[serde(default)]
    #[ts(optional)]
    pub env: Option<BTreeMap<String, String>>,
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
    /// Transport; an http URL must carry no credentials.
    pub transport: McpTransport,
    /// Absent = keep; `{}` = clear.
    #[serde(default)]
    #[ts(optional)]
    pub secret: Option<ServerSecret>,
    /// Enabled.
    pub enabled: bool,
    /// Trusted.
    pub trusted: bool,
    /// Allow a private-network address; ignored (stored as false) for stdio.
    pub allow_private: bool,
}
