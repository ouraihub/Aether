//! Backend connection snapshot shared by providers (producer) and chat (consumer),
//! so neither depends on the other (docs/10 §4.1).

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

/// Wire protocol of a model endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Protocol {
    /// Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
    /// OpenAI Chat Completions (`/v1/chat/completions`).
    OpenaiChat,
    /// OpenAI Responses (`/v1/responses`).
    OpenaiResponses,
}

/// Everything needed to call one model. Lives only in the backend: it is not
/// `Serialize`, and `Debug` never prints the key.
pub struct Connection {
    /// Endpoint base URL as configured (normalised by providers).
    pub base_url: String,
    /// Wire protocol.
    pub protocol: Protocol,
    /// Plaintext key, wiped on drop. `None` when no key is stored.
    pub api_key: Option<Zeroizing<String>>,
    /// Model id; `None` for endpoint-level use such as probing.
    pub model_id: Option<String>,
    /// Context window in tokens, when known.
    pub context_window: Option<u32>,
    /// Maximum output tokens, when known.
    pub max_output: Option<u32>,
    /// Whether the model accepts tool definitions; `None` = unknown.
    pub supports_tools: Option<bool>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("base_url", &crate::mask::mask_url(&self.base_url))
            .field("protocol", &self.protocol)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("model_id", &self.model_id)
            .field("context_window", &self.context_window)
            .field("max_output", &self.max_output)
            .field("supports_tools", &self.supports_tools)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_key() {
        let connection = Connection {
            base_url: "https://api.example.com/v1?key=leak-me".to_owned(),
            protocol: Protocol::OpenaiChat,
            api_key: Some(Zeroizing::new("sk-live-0123456789abcdefghij".to_owned())),
            model_id: Some("gpt-x".to_owned()),
            context_window: None,
            max_output: None,
            supports_tools: Some(true),
        };
        let printed = format!("{connection:?}");
        assert!(
            !printed.contains("sk-live") && !printed.contains("leak-me"),
            "{printed}"
        );
        assert!(printed.contains("gpt-x"));
    }
}
