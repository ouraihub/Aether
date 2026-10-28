//! Public conversation display projection (docs/22 §6), separate from stored messages.
use crate::conversations as domain;
use aether_chat::Message;
use aether_images::ImageRef;
use serde::Serialize;
use ts_rs::TS;

/// Metadata projected for the public history list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "ConversationSummary")]
pub struct UiConversationSummary {
    /// Conversation identity.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Optional host-defined scope, not an authorization claim.
    pub scope: Option<String>,
    /// Persisted completed context head.
    pub selected_turn_id: Option<String>,
    /// Creation time in Unix milliseconds.
    #[ts(type = "number")]
    pub created_at: i64,
    /// Last history update in Unix milliseconds.
    #[ts(type = "number")]
    pub updated_at: i64,
}
/// A display-only conversation, without native messages or database state.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "Conversation")]
pub struct UiConversation {
    /// Conversation metadata.
    pub summary: UiConversationSummary,
    /// All display candidates in sequence order.
    pub turns: Vec<UiTurn>,
}
/// Persisted turn state shown to consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "TurnStatus")]
pub enum UiTurnStatus {
    /// Reserved and not yet finalized.
    Running,
    /// Finished normally.
    Completed,
    /// Failed, including interrupted runs recovered on database open.
    Failed,
    /// Stopped by cancellation.
    Cancelled,
}
/// One display turn, with visible text and controlled image addresses.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "Turn")]
pub struct UiTurn {
    /// Turn identity.
    pub id: String,
    /// Previous context head.
    pub parent_id: Option<String>,
    /// Question identity shared by alternative candidates.
    pub question_id: String,
    /// Source candidate retried by this turn.
    pub supersedes_id: Option<String>,
    /// Reply or translation branch kind.
    #[ts(type = "'reply' | 'translation'")]
    pub kind: String,
    /// Persisted run state.
    pub status: UiTurnStatus,
    /// Authoritative user input.
    pub input: String,
    /// Joined visible assistant text, excluding tools and native reasoning.
    pub text: String,
    /// Model used, when known.
    pub model_id: Option<String>,
    /// Endpoint used, when known.
    pub endpoint_id: Option<String>,
    /// Terminal error code, when present.
    pub error_code: Option<String>,
    /// Provider-reported token counts; absent counts remain unknown.
    #[ts(optional = nullable)]
    pub usage: Option<aether_support::TokenUsage>,
    /// Saved context report, when available.
    #[ts(optional = nullable)]
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Requested reasoning effort.
    #[ts(optional = nullable)]
    pub reasoning_effort: Option<aether_support::ReasoningEffort>,
    /// Fixed cost saved at completion, never recalculated for display.
    #[ts(optional = nullable)]
    pub cost: Option<aether_support::Cost>,
    /// Safe display reasoning text, never raw native reasoning JSON.
    pub reasoning_text: Option<String>,
    /// Input images, retaining references with null addresses after expiry.
    pub images: Vec<ImageRef>,
    /// Generated images, not automatically replayed as model input.
    pub generated: Vec<ImageRef>,
}

/// Projects domain metadata into the public history-list shape.
pub fn summary(value: domain::ConversationSummary) -> UiConversationSummary {
    UiConversationSummary {
        id: value.id,
        title: value.title,
        scope: value.scope,
        selected_turn_id: value.selected_turn_id,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}
/// Projects domain turns into display-only text, usage and image references.
pub fn conversation(value: domain::Conversation) -> UiConversation {
    UiConversation {
        summary: summary(value.summary),
        turns: value.turns.into_iter().map(turn).collect(),
    }
}
/// Native display addresses; expired images retain their MIME with a null address.
fn image_ref(image: domain::TurnImage) -> ImageRef {
    ImageRef {
        url: image
            .data
            .map(|data| format!("data:{};base64,{data}", image.mime)),
        mime: image.mime,
    }
}
/// Projects one domain turn without exposing native reasoning or tool payloads.
pub fn turn(value: domain::Turn) -> UiTurn {
    let mut text = String::new();
    let mut reasoning_text = String::new();
    for message in &value.messages {
        if let Message::Assistant {
            text: body,
            reasoning,
            ..
        } = message
        {
            if !body.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(body);
            }
            for block in reasoning {
                match block.protocol {
                    aether_support::Protocol::OpenaiChat => {
                        if let Some(text) = block.value.as_str() {
                            reasoning_text.push_str(text);
                        }
                    }
                    aether_support::Protocol::AnthropicMessages => {
                        if block.value["type"] == "thinking"
                            && let Some(text) = block.value["thinking"].as_str()
                        {
                            reasoning_text.push_str(text);
                        }
                    }
                    aether_support::Protocol::OpenaiResponses => {
                        if let Some(parts) = block.value["summary"].as_array() {
                            for part in parts {
                                if part["type"] == "summary_text"
                                    && let Some(text) = part["text"].as_str()
                                {
                                    reasoning_text.push_str(text);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    UiTurn {
        id: value.id,
        parent_id: value.parent_id,
        question_id: value.question_id,
        supersedes_id: value.supersedes_id,
        kind: value.kind,
        status: match value.status {
            domain::TurnStatus::Running => UiTurnStatus::Running,
            domain::TurnStatus::Completed => UiTurnStatus::Completed,
            domain::TurnStatus::Failed => UiTurnStatus::Failed,
            domain::TurnStatus::Cancelled => UiTurnStatus::Cancelled,
        },
        input: value.input,
        text,
        model_id: value.model_id,
        endpoint_id: value.endpoint_id,
        error_code: value.error_code,
        usage: Some(aether_support::TokenUsage {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cache_read_tokens: value.cache_read_tokens,
            cache_write_tokens: value.cache_write_tokens,
            reasoning_tokens: value.reasoning_tokens,
        }),
        context_usage: value.context_usage,
        reasoning_effort: value.reasoning_effort,
        cost: value.cost,
        reasoning_text: Some(reasoning_text),
        images: value.images.into_iter().map(image_ref).collect(),
        generated: value.generated.into_iter().map(image_ref).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_image_keeps_reference_without_bytes() {
        let image = image_ref(domain::TurnImage {
            mime: "image/png".into(),
            data: None,
        });
        assert_eq!(image.mime, "image/png");
        assert_eq!(image.url, None);
    }
}
