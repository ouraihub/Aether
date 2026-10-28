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
    /// Anthropic Messages wire format.
    AnthropicMessages,
    /// OpenAI Chat Completions wire format.
    OpenaiChat,
    /// OpenAI Responses wire format.
    OpenaiResponses,
}

/// Everything needed to call one model. Lives only in the backend: it is not
/// `Serialize`, and `Debug` never prints the keys.
pub struct Connection {
    /// Canonical provider API base URL.
    pub base_url: String,
    /// Wire protocol.
    pub protocol: Protocol,
    /// Backend-only clear credentials in try order (rotated, enabled only); empty = none.
    pub api_keys: Vec<Zeroizing<String>>,
    /// Selected model identifier.
    pub model_id: Option<String>,
    /// Selected endpoint identifier.
    pub endpoint_id: Option<String>,
    /// Declared context window.
    pub context_window: Option<u32>,
    /// Declared output limit.
    pub max_output: Option<u32>,
    /// Declared local tool support.
    pub supports_tools: Option<bool>,
    /// Declared reasoning controls.
    pub reasoning: Option<crate::ReasoningConfig>,
    /// User-declared support for protocol-native web search.
    pub native_web_search: bool,
    /// User-declared image input support (docs/19 §1).
    pub can_vision: bool,
    /// User-declared prices for turn cost; `None` = unknown.
    pub pricing: Option<crate::ModelPricing>,
    /// Custom request headers of the endpoint, applied before system headers.
    pub headers: Vec<(String, String)>,
    /// Normalised image generation base URL; `None` = use `base_url`.
    pub image_base_url: Option<String>,
    /// Normalised image edit (`/images/edits`) base URL; `None` = use `base_url`.
    pub image_edit_base_url: Option<String>,
}

/// Header names only the system sets; saves reject them (docs/11 §3).
pub const RESERVED_HEADERS: [&str; 10] = [
    "authorization",
    "x-api-key",
    "anthropic-version",
    "host",
    "content-type",
    "content-length",
    "accept",
    "connection",
    "transfer-encoding",
    "cookie",
];

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("base_url", &crate::mask::mask_url(&self.base_url))
            .field("protocol", &self.protocol)
            .field("api_keys", &self.api_keys.len())
            .field("model_id", &self.model_id)
            .field("endpoint_id", &self.endpoint_id)
            .field("context_window", &self.context_window)
            .field("max_output", &self.max_output)
            .field("supports_tools", &self.supports_tools)
            .field("reasoning", &self.reasoning)
            .field("can_vision", &self.can_vision)
            .field("pricing", &self.pricing)
            .field("headers", &self.headers.len())
            .field(
                "image_base_url",
                &self.image_base_url.as_deref().map(crate::mask::mask_url),
            )
            .field(
                "image_edit_base_url",
                &self
                    .image_edit_base_url
                    .as_deref()
                    .map(crate::mask::mask_url),
            )
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
            api_keys: vec![Zeroizing::new("sk-live-0123456789abcdefghij".to_owned())],
            model_id: Some("gpt-x".to_owned()),
            endpoint_id: Some("e1".to_owned()),
            context_window: None,
            max_output: None,
            supports_tools: Some(true),
            reasoning: None,
            native_web_search: false,
            can_vision: false,
            pricing: None,
            headers: vec![("X-Trace".to_owned(), "secret-header".to_owned())],
            image_base_url: None,
            image_edit_base_url: None,
        };
        let printed = format!("{connection:?}");
        assert!(
            !printed.contains("sk-live")
                && !printed.contains("leak-me")
                && !printed.contains("secret-header"),
            "{printed}"
        );
        assert!(printed.contains("gpt-x"));
    }
}
