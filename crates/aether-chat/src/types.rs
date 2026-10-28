//! Run input, tool contract and run events (docs/12 §2, §4, §5).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub use aether_support::{Approval, PermissionMode, ToolEffect};
use aether_support::{Connection, KitError, ReasoningBlock, ReasoningEffort};

use aether_images::ImagePart;
/// Images per user input (docs/19 §2).
pub const MAX_IMAGES: usize = 4;

/// Everything one run needs, assembled by the integration layer before the run starts.
pub struct RunInput {
    /// Run id reported to the UI.
    pub run_id: String,
    /// App-defined label of the run (`RunRequest.purpose`); passed to `authorize` as is.
    pub purpose: String,
    /// Fixed for the whole run; decides the catalogue and the proposed approvals (§5.1).
    pub permission_mode: PermissionMode,
    /// Resolved connection snapshot (from providers).
    pub connection: Connection,
    /// System prompt parts, concatenated in order.
    pub system: Vec<SystemPart>,
    /// Completed turns read from the store and trimmed.
    pub history: Vec<Message>,
    /// This turn's user input.
    pub input: String,
    /// This turn's images; empty for text-only input.
    pub images: Vec<ImagePart>,
    /// Tool catalogue, fixed for the whole run.
    pub tools: Vec<ToolSpec>,
    /// Limits.
    pub budget: Budget,
    /// Requested reasoning effort.
    pub reasoning_effort: ReasoningEffort,
    /// Use the configured protocol's server-side search, never a local tool.
    pub native_web_search: bool,
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
    /// What the tool does; MCP tools are always [`ToolEffect::Other`].
    pub effect: ToolEffect,
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
        /// Structured data for the UI only; never model instructions.
        payload: Option<Value>,
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
    /// User text with images; only built by `trim_history`, never stored in `turns.messages`.
    UserImages {
        /// Text.
        text: String,
        /// Images, in order.
        images: Vec<ImagePart>,
    },
    /// Assistant text plus the tool calls it requested.
    Assistant {
        /// Text.
        text: String,
        /// Requested tool calls, in order.
        tool_calls: Vec<ToolCall>,
        /// Provider-native reasoning blocks.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reasoning: Vec<ReasoningBlock>,
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
    /// Known provider usage; absent values are unknown, not zero.
    Usage {
        /// Total input tokens.
        input_tokens: Option<u32>,
        /// Total output tokens.
        output_tokens: Option<u32>,
        /// Input cache reads.
        cache_read_tokens: Option<u32>,
        /// Input cache writes.
        cache_write_tokens: Option<u32>,
        /// Reasoning output tokens.
        reasoning_tokens: Option<u32>,
        /// Turn cost from the model's pricing; `None` = unknown.
        cost: Option<aether_support::Cost>,
    },
    /// Provider context usage for the current request.
    Context {
        /// Last request context report or estimate.
        usage: aether_support::ContextUsage,
    },
    /// Provider reasoning text delta.
    ReasoningDelta {
        /// Displayable reasoning fragment.
        text: String,
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

/// Token usage reported by the upstream; `None` when not reported (never estimated).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: Option<u32>,
    /// Output tokens.
    pub output_tokens: Option<u32>,
    /// Cached input tokens.
    pub cache_read_tokens: Option<u32>,
    /// Cached write tokens.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning tokens.
    pub reasoning_tokens: Option<u32>,
}

#[cfg(feature = "native")]
impl Usage {
    /// Sums rounds; a side stays `None` only if no round reported it.
    pub(crate) fn add(&mut self, round: Self) {
        let sum = |a: Option<u32>, b: Option<u32>| match (a, b) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (a, b) => a.or(b),
        };
        self.input_tokens = sum(self.input_tokens, round.input_tokens);
        self.output_tokens = sum(self.output_tokens, round.output_tokens);
        self.cache_read_tokens = sum(self.cache_read_tokens, round.cache_read_tokens);
        self.cache_write_tokens = sum(self.cache_write_tokens, round.cache_write_tokens);
        self.reasoning_tokens = sum(self.reasoning_tokens, round.reasoning_tokens);
    }
}

impl From<Usage> for aether_support::TokenUsage {
    fn from(u: Usage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_tokens: u.cache_read_tokens,
            cache_write_tokens: u.cache_write_tokens,
            reasoning_tokens: u.reasoning_tokens,
        }
    }
}

/// Outcome and known usage of a completed or interrupted run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    /// Terminal status.
    pub status: EndStatus,
    /// Produced messages, including partial output.
    pub messages: Vec<Message>,
    /// Known provider-reported token counts.
    pub usage: Usage,
    /// Cost summed per round from the connection's pricing; `None` = unknown (docs/41 §4).
    pub cost: Option<aether_support::Cost>,
    /// Last request context estimate or report.
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Tool loop stopped at its configured round limit.
    pub exhausted: bool,
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
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost: Some(aether_support::Cost {
                amount: 0.5,
                currency: aether_support::Currency::Usd,
            }),
        };
        assert_eq!(
            wire(&usage),
            r#"{"kind":"usage","inputTokens":3,"outputTokens":null,"cacheReadTokens":null,"cacheWriteTokens":null,"reasoningTokens":null,"cost":{"amount":0.5,"currency":"USD"}}"#,
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
