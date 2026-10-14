//! Shared reasoning and token/context contracts (docs/18).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::Protocol;

/// Explicit per-run reasoning choice; default leaves provider controls untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ReasoningEffort {
    /// Provider default.
    #[default]
    Default,
    /// Explicitly disabled reasoning.
    None,
    /// Minimal effort.
    Minimal,
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
    /// Extra-high effort.
    Xhigh,
    /// Maximum declared effort.
    Max,
}

/// Provider-specific reasoning request dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum ReasoningDialect {
    /// OpenAI effort parameter.
    Openai,
    /// Anthropic token budget.
    AnthropicBudget,
    /// Anthropic adaptive effort.
    AnthropicAdaptive,
}

/// User-declared reasoning capabilities of one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ReasoningConfig {
    /// Request parameter dialect.
    pub dialect: ReasoningDialect,
    /// Allowed explicit choices.
    pub efforts: Vec<ReasoningEffort>,
    /// Upper bound for budget-based reasoning.
    #[serde(default)]
    #[ts(optional)]
    pub budget_tokens: Option<u32>,
}

impl ReasoningConfig {
    /// Checks protocol compatibility and bounded nonduplicate effort declarations.
    pub fn validate(&self, protocol: Protocol, max_output: Option<u32>) -> bool {
        if self.efforts.is_empty()
            || self
                .efforts
                .iter()
                .any(|e| matches!(e, ReasoningEffort::Default))
        {
            return false;
        }
        let mut seen = HashSet::new();
        if self.efforts.iter().any(|effort| !seen.insert(*effort)) {
            return false;
        }
        let protocol_ok = match self.dialect {
            ReasoningDialect::Openai => {
                matches!(protocol, Protocol::OpenaiChat | Protocol::OpenaiResponses)
            }
            ReasoningDialect::AnthropicBudget | ReasoningDialect::AnthropicAdaptive => {
                protocol == Protocol::AnthropicMessages
            }
        };
        if !protocol_ok {
            return false;
        }
        match self.dialect {
            ReasoningDialect::AnthropicBudget => self
                .budget_tokens
                .is_some_and(|n| n >= 1024 && max_output.is_none_or(|m| n < m)),
            ReasoningDialect::Openai | ReasoningDialect::AnthropicAdaptive => {
                self.budget_tokens.is_none()
            }
        }
    }

    /// Tests whether this declared capability permits the requested effort.
    pub fn allows(&self, effort: ReasoningEffort) -> bool {
        matches!(effort, ReasoningEffort::Default) || self.efforts.contains(&effort)
    }
}

/// Provider-native reasoning payload retained for exact replay, never displayed verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningBlock {
    /// Origin wire protocol.
    pub protocol: Protocol,
    /// Origin endpoint identity.
    pub endpoint_id: Option<String>,
    /// Origin model identity.
    pub model_id: Option<String>,
    /// Provider-native block, never displayed as raw JSON.
    #[ts(type = "unknown")]
    pub value: Value,
}

/// Known token counts; missing values remain unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TokenUsage {
    /// Total input tokens including cache.
    pub input_tokens: Option<u32>,
    /// Total output tokens including reasoning.
    pub output_tokens: Option<u32>,
    /// Input cache-read detail.
    pub cache_read_tokens: Option<u32>,
    /// Input cache-write detail.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning output detail.
    pub reasoning_tokens: Option<u32>,
}

/// Provenance of the current request context count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ContextSource {
    /// Provider usage report.
    Reported,
    /// Character-budget estimate.
    Estimated,
    /// No usable count.
    Unknown,
}

/// Context count and identity of the most recent model request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextUsage {
    /// Input plus output count.
    pub tokens: Option<u32>,
    /// Configured real model window, not a fallback safety budget.
    pub context_window: Option<u32>,
    /// Count provenance.
    pub source: ContextSource,
    /// Endpoint used for the count.
    pub endpoint_id: Option<String>,
    /// Model used for the count.
    pub model_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_budget_contract() {
        let c = ReasoningConfig {
            dialect: ReasoningDialect::AnthropicBudget,
            efforts: vec![ReasoningEffort::Low],
            budget_tokens: Some(1024),
        };
        assert!(c.validate(Protocol::AnthropicMessages, Some(4096)));
        assert!(!c.validate(Protocol::OpenaiChat, Some(4096)));
    }
}
