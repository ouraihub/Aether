//! `KitHost` and the run request it decides on (docs/00 §5.1, docs/15 §3).

pub use aether_support::KitError;

#[cfg(feature = "chat")]
pub use chat_contract::*;

/// The only trait a host implements. Every method has a default, so hosts that
/// only use providers or the MCP server write none of them.
#[async_trait::async_trait]
pub trait KitHost: Send + Sync + 'static {
    /// Which model this run uses; roles and fallbacks are host policy.
    /// Default: the model the UI sent, otherwise `needs_config`.
    ///
    /// # Errors
    /// Whatever the host decides; the run is not started.
    #[cfg(feature = "chat")]
    fn select_model(&self, req: &RunRequest) -> Result<ModelRef, KitError> {
        default_select(req)
    }
    /// Host system prompt.
    #[cfg(feature = "chat")]
    fn system_prompt(&self, _req: &RunRequest) -> Option<String> {
        None
    }
    /// Context for `req.context`; called in `spawn_blocking`.
    ///
    /// # Errors
    /// Whatever the host decides; the run is not started.
    #[cfg(feature = "chat")]
    fn context(&self, _req: &RunRequest) -> Result<Option<ContextBlock>, KitError> {
        Ok(None)
    }
    /// Host tools allowed for this run.
    #[cfg(feature = "chat")]
    fn tools(&self, _req: &RunRequest) -> Vec<HostTool> {
        Vec::new()
    }
    /// Runs a host tool.
    ///
    /// # Errors
    /// A [`ToolError`]; `unknown = true` when side effects may have happened.
    #[cfg(feature = "chat")]
    async fn call_tool(
        &self,
        _call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        Err(ToolError::not_found())
    }
    /// Final say before execution, for host and MCP tools alike.
    #[cfg(feature = "chat")]
    fn authorize(&self, _call: &ToolCall, default: Approval) -> Approval {
        default
    }
}

#[cfg(feature = "chat")]
mod chat_contract {
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use ts_rs::TS;

    pub use aether_chat::{Approval, ToolCall, ToolSource, ToolSpec};
    use aether_providers::KitError;
    pub use aether_providers::ModelRef;
    pub use aether_support::{ContextUsage, ReasoningEffort, TokenUsage};

    pub use aether_support::SkillRef;
    // SkillRef comes from aether-support so `chat` without the `skills` feature still accepts
    // (and rejects non-empty) skill selections (docs/15 §4 step 3).
    pub use tokio_util::sync::CancellationToken;

    /// Business input of one run, sent by the UI (`chat_run`).
    #[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    #[ts(export)]
    pub struct RunRequest {
        /// App-defined label such as `chat` or `polish`; aether does not interpret it.
        pub purpose: String,
        /// Model picked in the UI.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub model: Option<ModelRef>,
        /// Skills pinned to the revisions the user confirmed.
        pub skills: Vec<SkillRef>,
        /// App-defined context reference.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub context: Option<ContextRef>,
        /// User input of this turn.
        pub input: String,
        /// Conversation to continue; absent = new conversation.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub conversation_id: Option<String>,
        /// History scope.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub scope: Option<String>,
        /// Explicit one-shot pre-search before model execution.
        #[serde(default)]
        #[ts(optional)]
        pub web_search: Option<bool>,
        /// Retry the selected reply, preserving the source candidate.
        pub regenerate: bool,
        /// Selected reply whose ancestry supplies the next input's context.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub reply_to_id: Option<String>,
        /// Reply to generate another candidate for.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub retry_turn_id: Option<String>,
        /// Reply to translate with the normal model runner.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub translate_turn_id: Option<String>,
        /// Requested translation language.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub target_language: Option<String>,
        /// UI language, e.g. `zh-CN`; selects fixed prompt texts.
        pub language: String,
        /// Requested reasoning effort; omitted uses the endpoint default.
        #[serde(default)]
        #[ts(optional)]
        pub reasoning_effort: Option<ReasoningEffort>,
    }

    /// Reference to host data; `kind` and `id` are defined by the app.
    #[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    #[ts(export)]
    pub struct ContextRef {
        /// App-defined kind.
        pub kind: String,
        /// App-defined id.
        pub id: String,
    }

    /// Returned by `chat_run` once the run is accepted; results arrive as events.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
    #[serde(rename_all = "camelCase")]
    #[ts(export)]
    pub struct RunAccepted {
        /// Run id for `chat_cancel` / `chat_approve`.
        pub run_id: String,
        /// Conversation the turn belongs to; `null` without a store.
        pub conversation_id: Option<String>,
        /// Stable stored candidate id; `null` without a store.
        pub turn_id: Option<String>,
    }

    /// `{ ok: true }` reply of commands that only acknowledge.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
    #[ts(export)]
    pub struct Ack {
        /// Always `true`.
        pub ok: bool,
    }

    /// Host context for one run, placed in the system prompt.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ContextBlock {
        /// Source label shown to the model.
        pub source: String,
        /// Context text; truncated to the budget.
        pub text: String,
    }

    /// A host tool offered to the model; same shape as Chat's [`ToolSpec`].
    #[derive(Debug, Clone, PartialEq)]
    pub struct HostTool {
        /// Catalogue entry (`source` is `Host`).
        pub spec: ToolSpec,
    }

    /// Successful host tool result.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ToolOutput {
        /// Text fed back to the model.
        pub text: String,
        /// Structured data for the host UI.
        pub payload: Option<Value>,
    }

    /// Failed host tool call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ToolError {
        /// Machine-readable code.
        pub code: String,
        /// Message fed back to the model.
        pub message: String,
        /// The call may have had side effects; it is reported as `unknown`, never retried.
        pub unknown: bool,
    }

    impl ToolError {
        /// The host has no tool with this name.
        #[must_use]
        pub fn not_found() -> Self {
            Self {
                code: "not_found".to_owned(),
                message: "tool not found".to_owned(),
                unknown: false,
            }
        }
    }

    /// Default model selection: the model the UI sent.
    ///
    /// # Errors
    /// `needs_config{reason:"model"}` when the request has no model.
    pub fn default_select(req: &RunRequest) -> Result<ModelRef, KitError> {
        req.model
            .clone()
            .ok_or_else(|| KitError::needs_config("model"))
    }
}
