//! Run input, tool contract and run events (docs/12 §2, §4, §5).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use aether_support::{Connection, KitError};

pub use aether_support::Approval;

/// Everything one run needs, assembled by the integration layer before the run starts.
#[derive(Debug)]
pub struct RunInput {
    /// Run id reported to the UI.
    pub run_id: String,
    /// Resolved connection snapshot (from providers).
    pub connection: Connection,
    /// System prompt parts, concatenated in order.
    pub system: Vec<SystemPart>,
    /// Completed turns read from the store and trimmed.
    pub history: Vec<Message>,
    /// This turn's user input.
    pub input: String,
    /// Tool catalogue, fixed for the whole run.
    pub tools: Vec<ToolSpec>,
    /// Limits.
    pub budget: Budget,
}

/// One part of the system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemPart {
    /// Host prompt, verbatim.
    Host(String),
    /// Selected skill, wrapped in an external-content boundary.
    Skill {
        /// Skill id.
        id: String,
        /// Pinned revision.
        revision: u32,
        /// Skill name.
        name: String,
        /// Skill body.
        body: String,
    },
    /// Host context block.
    Context {
        /// Source label.
        source: String,
        /// Context text.
        text: String,
    },
}

/// Size and round limits of one run; defaults per docs/12 §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Maximum output tokens.
    pub max_output: u32,
    /// Context window in tokens.
    pub context_window: u32,
    /// Maximum model rounds.
    pub max_rounds: u8,
    /// Tool output is truncated to this many characters.
    pub tool_output_chars: usize,
    /// Combined body length of the selected skills.
    pub skills_chars: usize,
    /// Host context is truncated to this many characters.
    pub context_chars: usize,
}

impl Default for Budget {
    /// docs/12 §2 defaults for a model whose limits are unknown.
    fn default() -> Self {
        Self {
            max_output: 4096,
            context_window: 32_000,
            max_rounds: 5,
            tool_output_chars: 8_000,
            skills_chars: 48_000,
            context_chars: 24_000,
        }
    }
}

impl Budget {
    /// `self` with the model limits taken from the connection when known.
    #[must_use]
    pub fn for_connection(self, connection: &Connection) -> Self {
        Self {
            max_output: connection.max_output.unwrap_or(self.max_output),
            context_window: connection.context_window.unwrap_or(self.context_window),
            ..self
        }
    }
}

/// A tool in the run catalogue.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// `^[a-zA-Z0-9_-]{1,64}$`.
    pub name: String,
    /// At most 1024 characters.
    pub description: String,
    /// JSON Schema of the arguments (an object schema).
    pub parameters: Value,
    /// Catalogue default; the host still decides before execution.
    pub approval: Approval,
    /// Where the tool runs; used for display and external-content wrapping.
    pub source: ToolSource,
}

/// Origin of a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Registered by the host.
    Host,
    /// Provided by an MCP server.
    Mcp {
        /// Server display name.
        server: String,
    },
}

/// A complete tool call from the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Call id, unique within a run.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments (a JSON object).
    pub args: Value,
}

/// Result of executing one tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolExecution {
    /// Finished.
    Ok {
        /// Text fed back to the model.
        text: String,
        /// Structured data for the UI only.
        payload: Option<Value>,
    },
    /// Failed; fed back so the model can fix its arguments.
    Error {
        /// Machine-readable code.
        code: String,
        /// Message fed back to the model.
        message: String,
    },
    /// Started but the outcome is unknown; side effects may have happened.
    Unknown {
        /// Why the outcome is unknown.
        reason: String,
    },
}

/// One message of a turn as stored and replayed. Serde only for `chat.db`, not exported to TS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    /// User text.
    User(String),
    /// Assistant text plus the tool calls it requested.
    Assistant {
        /// Text.
        text: String,
        /// Requested tool calls, in order.
        tool_calls: Vec<ToolCall>,
    },
    /// Result of one tool call, as fed back to the model.
    Tool {
        /// Call id.
        call_id: String,
        /// Text fed back to the model.
        text: String,
    },
}

/// Display source of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToolSourceKind {
    /// Host tool.
    Host,
    /// MCP tool.
    Mcp,
}

/// Outcome of a tool call as shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToolOutcome {
    /// Finished.
    Ok,
    /// Failed.
    Error,
    /// Denied by the host or the user.
    Denied,
    /// Started; result unknown, may have taken effect.
    Unknown,
}

/// Terminal status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EndStatus {
    /// Finished normally (possibly with `exhausted`).
    Completed,
    /// Failed after being accepted.
    Failed,
    /// Cancelled.
    Cancelled,
}

/// A search result source shown in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Result summary.
    pub snippet: String,
}

/// Streamed to the UI; every run ends with exactly one `end`.
#[derive(Debug, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum RunEvent {
    /// Search starts before the model request.
    SearchStart,
    /// Search results returned for the current query.
    SearchResult {
        /// User query sent to the provider.
        query: String,
        /// Provider id.
        provider: String,
        /// Search hits.
        hits: Vec<SearchHit>,
    },
    /// A model round starts.
    RoundStart {
        /// 1-based round number.
        round: u8,
    },
    /// Streamed text.
    TextDelta {
        /// Text chunk.
        text: String,
    },
    /// A complete, catalogued call is about to be authorized.
    ToolCall {
        /// Call id.
        call_id: String,
        /// Tool name.
        name: String,
        /// Tool source.
        source: ToolSourceKind,
        /// MCP server name.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        server: Option<String>,
    },
    /// The user must allow or deny the call (`Ask` only).
    ApprovalRequired {
        /// Call id.
        call_id: String,
        /// Tool name.
        name: String,
        /// Arguments preview, at most 2000 characters.
        args_preview: String,
    },
    /// A call finished.
    ToolResult {
        /// Call id.
        call_id: String,
        /// Outcome.
        outcome: ToolOutcome,
        /// One-line summary.
        summary: String,
        /// Structured data for the host UI.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional, type = "unknown")]
        payload: Option<Value>,
    },
    /// Token usage; `null` when the upstream does not report it.
    Usage {
        /// Input tokens.
        input_tokens: Option<u32>,
        /// Output tokens.
        output_tokens: Option<u32>,
    },
    /// The single terminal event.
    End {
        /// Terminal status.
        status: EndStatus,
        /// Set when `status` is `failed`.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        error: Option<KitError>,
        /// `true` when the round limit stopped the tool loop.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exhausted: Option<bool>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(clippy::unwrap_used, reason = "test")]
    fn wire(event: &RunEvent) -> String {
        serde_json::to_string(event).unwrap()
    }

    // The UI switches on `kind` and reads camelCase fields (docs/12 §4).
    #[test]
    fn run_events_match_the_ts_contract() {
        let call = RunEvent::ToolCall {
            call_id: "c1".into(),
            name: "search".into(),
            source: ToolSourceKind::Mcp,
            server: Some("maps".into()),
        };
        assert_eq!(
            wire(&call),
            r#"{"kind":"tool_call","callId":"c1","name":"search","source":"mcp","server":"maps"}"#
        );
        let usage = RunEvent::Usage {
            input_tokens: Some(3),
            output_tokens: None,
        };
        assert_eq!(
            wire(&usage),
            r#"{"kind":"usage","inputTokens":3,"outputTokens":null}"#
        );
        let end = RunEvent::End {
            status: EndStatus::Failed,
            error: Some(KitError::cancelled()),
            exhausted: None,
        };
        assert_eq!(
            wire(&end),
            r#"{"kind":"end","status":"failed","error":{"code":"cancelled","message":"cancelled"}}"#
        );
    }
}
