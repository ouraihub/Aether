//! Host policy seam; no UI or transport dependencies.

use crate::RunRequest;
pub use aether_chat::{
    Approval, AuthorizeContext, PermissionMode, ToolCall, ToolEffect, ToolSource, ToolSpec,
};
pub use aether_providers::ModelRef;
use aether_support::KitError;
pub use aether_support::{ContextUsage, ReasoningEffort, SkillRef, TokenUsage};
pub use tokio_util::sync::CancellationToken;

/// Host-provided context, wrapped separately from trusted system instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBlock {
    /// Source label.
    pub source: String,
    /// Context body.
    pub text: String,
}

/// Host tool catalogue entry.
#[derive(Debug, Clone, PartialEq)]
pub struct HostTool {
    /// Fixed specification.
    pub spec: ToolSpec,
}

/// Successful host tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Model-visible text.
    pub text: String,
    /// Host-approved functional data.
    pub payload: Option<serde_json::Value>,
}

/// Host tool failure; unknown side effects must never be automatically retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    /// Machine-readable code.
    pub code: String,
    /// Model-visible message.
    pub message: String,
    /// Side effects may already have occurred.
    pub unknown: bool,
}

impl ToolError {
    /// Missing host tool.
    #[must_use]
    pub fn not_found() -> Self {
        Self {
            code: "not_found".into(),
            message: "tool not found".into(),
            unknown: false,
        }
    }
}

/// Explicit model selection, otherwise configuration is required.
///
/// # Errors
/// Returns `needs_config` if no model was supplied.
pub fn default_select(request: &RunRequest) -> Result<ModelRef, KitError> {
    request
        .model
        .clone()
        .ok_or_else(|| KitError::needs_config("model"))
}

/// Trusted application policy. Synchronous preparation methods run on the blocking pool.
#[async_trait::async_trait]
pub trait KitHost: Send + Sync + 'static {
    /// Selects the model from the effective action input.
    ///
    /// # Errors
    /// Rejects unavailable model selection or host policy failures before acceptance.
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, KitError> {
        default_select(request)
    }
    /// Supplies trusted system instructions.
    fn system_prompt(&self, _request: &RunRequest) -> Option<String> {
        None
    }
    /// Resolves host-owned context with application authorization.
    ///
    /// # Errors
    /// Returns host authorization or context lookup failures before acceptance.
    fn context(&self, _request: &RunRequest) -> Result<Option<ContextBlock>, KitError> {
        Ok(None)
    }
    /// Fixes the tool catalogue and run-specific capabilities.
    fn tools(&self, _request: &RunRequest, _run_id: &str) -> Vec<HostTool> {
        Vec::new()
    }
    /// Releases run capabilities exactly once, after all tool calls and finalization.
    fn run_finished(&self, _run_id: &str) {}
    /// Modes this application allows for this effective request.
    fn permission_modes(&self, _request: &RunRequest) -> Vec<PermissionMode> {
        PermissionMode::BASELINE.to_vec()
    }
    /// False prevents even opening an MCP session.
    fn mcp_tools(&self, _request: &RunRequest) -> bool {
        true
    }
    /// Independent painting model, when configured.
    fn painting_model(&self) -> Option<ModelRef> {
        None
    }
    /// Executes a host tool; cooperate with cancellation and report uncertain effects.
    ///
    /// # Errors
    /// Reports a confirmed failure or an unknown side-effect outcome through `ToolError`.
    async fn call_tool(
        &self,
        _call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        Err(ToolError::not_found())
    }
    /// Fast, synchronous final policy decision; catalogue Deny remains absolute.
    fn authorize(&self, _call: &ToolCall, context: &AuthorizeContext<'_>) -> Approval {
        context.proposed
    }
}
