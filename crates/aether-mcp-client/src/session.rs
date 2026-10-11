//! The per-run tool catalogue and calls (docs/13 §3, §4).

use std::time::Duration;

use aether_support::KitError;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::Approval;
use crate::protocol::{Connection, RpcError};
use crate::store::Target;

/// One MCP tool in the run catalogue.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    /// Server display name.
    pub server: String,
    /// Server slug.
    pub slug: String,
    /// Tool name on the server.
    pub name: String,
    /// `mcp_<slug>_<name>`, the name the model sees.
    pub prefixed: String,
    /// Description (≤ 1024 characters).
    pub description: String,
    /// JSON schema of the arguments.
    pub schema: Value,
    /// Per-tool default; the host still has the final say.
    pub approval: Approval,
}

/// Outcome of one MCP tool call; the integration layer maps it to Chat's `ToolExecution`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCallResult {
    /// Text content joined; non-text items are placeholders.
    Ok {
        /// The joined text.
        text: String,
    },
    /// Not executed, or the tool reported `isError`.
    Error {
        /// `tool_error` for `isError`, `unknown_tool`, or a `KitErrorCode` in snake case.
        code: String,
        /// Safe message.
        message: String,
    },
    /// The request was sent but no answer arrived: it may have run.
    Unknown {
        /// `timeout`, `network`, `cancelled` or `budget_exceeded`.
        reason: String,
    },
}

pub(crate) struct SessionServer {
    pub(crate) target: Target,
    /// Set when discovery already connected; otherwise connected on the first call.
    pub(crate) connection: Option<Connection>,
    pub(crate) tools: Vec<McpTool>,
}

/// Connections reused for one run, one per server, opened on demand.
pub struct McpSession {
    servers: Vec<SessionServer>,
    timeout: Duration,
}

impl McpSession {
    pub(crate) const fn new(servers: Vec<SessionServer>, timeout: Duration) -> Self {
        Self { servers, timeout }
    }

    /// The catalogue fixed at the start of the run.
    #[must_use]
    pub fn tools(&self) -> Vec<McpTool> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }

    /// Calls a tool by its prefixed name.
    pub async fn call(
        &mut self,
        prefixed: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> McpCallResult {
        let timeout = self.timeout;
        let found = self.servers.iter_mut().find_map(|server| {
            let name = server
                .tools
                .iter()
                .find(|tool| tool.prefixed == prefixed)?
                .name
                .clone();
            Some((server, name))
        });
        let Some((server, tool)) = found else {
            return McpCallResult::Error {
                code: "unknown_tool".to_owned(),
                message: format!("unknown tool {prefixed}"),
            };
        };
        let mut connection = match server.connection.take() {
            Some(connection) => connection,
            // Nothing was called yet, so a failed handshake is a plain error.
            None => match server.target.connect(timeout, cancel).await {
                Ok(connection) => connection,
                Err(error) => return failed(&error),
            },
        };
        let params = json!({ "name": tool, "arguments": args });
        let result = connection.call("tools/call", params, cancel).await;
        server.connection = Some(connection);
        match result {
            Ok(result) => {
                let text = content_text(&result);
                if result["isError"].as_bool() == Some(true) {
                    McpCallResult::Error {
                        code: "tool_error".to_owned(),
                        message: text,
                    }
                } else {
                    McpCallResult::Ok { text }
                }
            }
            Err(RpcError::Unknown(error)) => McpCallResult::Unknown {
                reason: code_str(&error),
            },
            Err(error @ (RpcError::Failed(_) | RpcError::Expired)) => failed(&error.into_error()),
        }
    }
}

fn failed(error: &KitError) -> McpCallResult {
    McpCallResult::Error {
        code: code_str(error),
        message: error.message().to_owned(),
    }
}

fn code_str(error: &KitError) -> String {
    serde_json::to_value(error.code())
        .ok()
        .and_then(|code| code.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `text` items joined by newlines; other types become `[unsupported content: <type>]`.
fn content_text(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| match item["type"].as_str() {
            Some("text") => item["text"].as_str().unwrap_or_default().to_owned(),
            other => format!("[unsupported content: {}]", other.unwrap_or("unknown")),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
