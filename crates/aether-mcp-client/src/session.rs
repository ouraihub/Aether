//! Fixed per-run tool catalogue and lossless bounded call results (docs/27).

use aether_support::KitError;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::Approval;
use crate::protocol::{Connection, RpcError, tool_result};
use crate::runtime::{Timeouts, check_cancel, connect};
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

/// Transport-neutral outcome; model projection belongs to the application runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCallResult {
    /// A valid response, including tool-level errors and extension content.
    Completed {
        /// Original content entries, collectively bounded by the transport budget.
        content: Vec<Value>,
        /// Original structured result object, when present.
        structured_content: Option<Value>,
        /// Tool-level failure does not mean that execution never happened.
        is_error: bool,
    },
    /// A definite pre-dispatch failure or a valid JSON-RPC error.
    Failed {
        /// Safe error code.
        code: String,
        /// Safe error message.
        message: String,
    },
    /// Dispatched without a trustworthy terminal response; never automatically replay.
    Unknown {
        /// timeout/network/cancelled/budget_exceeded/invalid_response.
        reason: String,
    },
}

pub(crate) struct SessionServer {
    pub(crate) target: Target,
    /// Set when discovery already connected; otherwise connected on the first call.
    pub(crate) connection: Option<Connection>,
    pub(crate) tools: Vec<McpTool>,
}

/// Connections reused for one run, one per server, opened on demand. A stdio server
/// process lives as long as its connection: dropping the session ends it.
pub struct McpSession {
    servers: Vec<SessionServer>,
    timeouts: Timeouts,
    shutdown: CancellationToken,
}

impl McpSession {
    pub(crate) const fn new(
        servers: Vec<SessionServer>,
        timeouts: Timeouts,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            servers,
            timeouts,
            shutdown,
        }
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
        if let Err(error) = check_cancel(cancel, &self.shutdown) {
            return failed(&error);
        }
        let timeouts = self.timeouts;
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
            return McpCallResult::Failed {
                code: "unknown_tool".to_owned(),
                message: format!("unknown tool {prefixed}"),
            };
        };
        // A stdio process that exited or was ended after a lost answer is restarted.
        let mut connection = match server.connection.take().filter(Connection::is_alive) {
            Some(connection) => connection,
            // Nothing was called yet, so a failed handshake is a plain error.
            None => match connect(&server.target, timeouts, cancel, &self.shutdown).await {
                Ok(connection) => connection,
                Err(error) => return failed(&error),
            },
        };
        let params = json!({ "name": tool, "arguments": args });
        let result = connection
            .call("tools/call", params, cancel, &self.shutdown)
            .await;
        server.connection = Some(connection);
        match result {
            Ok(result) => tool_result(result),
            Err(RpcError::Unknown(error)) => McpCallResult::Unknown {
                reason: if error.details().reason() == Some("invalid_response") {
                    "invalid_response".to_owned()
                } else {
                    code_str(&error)
                },
            },
            Err(error @ (RpcError::Failed(_) | RpcError::Expired)) => failed(&error.into_error()),
        }
    }
}

fn failed(error: &KitError) -> McpCallResult {
    McpCallResult::Failed {
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
