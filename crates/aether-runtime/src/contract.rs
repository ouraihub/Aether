//! Public JSON bindings; field contracts are defined in docs/22.

use serde::Serialize;
use ts_rs::TS;

#[cfg(feature = "conversations")]
pub mod conversation_dto;

/// Successful acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct Ack {
    /// Always true.
    #[ts(type = "true")]
    pub ok: bool,
}

#[cfg(feature = "runs")]
pub use runs::*;

#[cfg(feature = "runs")]
mod runs {
    use aether_providers::ModelRef;
    use aether_support::{KitError, PermissionMode, ReasoningEffort, SkillRef};
    use serde::{Deserialize, Serialize};
    use ts_rs::TS;

    /// App-defined context reference, not an authorization credential.
    #[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    #[ts(export)]
    pub struct ContextRef {
        /// App-defined kind.
        pub kind: String,
        /// App-defined identifier.
        pub id: String,
    }

    /// Business input of a run, with no transport or owner identity.
    #[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    #[ts(export)]
    pub struct RunRequest {
        /// App-defined purpose.
        pub purpose: String,
        /// Explicit model selection.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub model: Option<ModelRef>,
        /// Pinned skills.
        pub skills: Vec<SkillRef>,
        /// Host context selection.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub context: Option<ContextRef>,
        /// Current input, overridden by an authoritative action source.
        pub input: String,
        /// Current attached images.
        #[serde(default)]
        #[ts(as = "Option<Vec<aether_images::ImagePart>>", optional)]
        pub images: Vec<aether_images::ImagePart>,
        /// Conversation to continue.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub conversation_id: Option<String>,
        /// Host-defined history scope.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub scope: Option<String>,
        /// Explicit pre-search intent.
        #[serde(default)]
        #[ts(optional)]
        pub web_search: Option<bool>,
        /// Retry the selected candidate.
        pub regenerate: bool,
        /// Explicit history head.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub reply_to_id: Option<String>,
        /// Source candidate to retry.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub retry_turn_id: Option<String>,
        /// Source candidate to translate.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub translate_turn_id: Option<String>,
        /// Translation target language.
        #[serde(default)]
        #[ts(optional = nullable)]
        pub target_language: Option<String>,
        /// UI language.
        pub language: String,
        /// Requested reasoning effort.
        #[serde(default)]
        #[ts(optional)]
        pub reasoning_effort: Option<ReasoningEffort>,
        /// Offer image generation.
        #[serde(default)]
        #[ts(optional)]
        pub generate_image: Option<bool>,
        /// Host-authorized permission mode.
        #[serde(default)]
        #[ts(optional)]
        pub permission_mode: Option<PermissionMode>,
    }

    /// Accepted identity; events may arrive before this reply is delivered.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
    #[serde(rename_all = "camelCase")]
    #[ts(export)]
    pub struct RunAccepted {
        /// Service-generated identifier.
        pub run_id: String,
        /// Persisted conversation, when enabled.
        pub conversation_id: Option<String>,
        /// Persisted candidate, when enabled.
        pub turn_id: Option<String>,
    }

    /// Public run lifecycle state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(export)]
    pub enum RunStatus {
        /// Executing, awaiting approval or finalizing.
        Running,
        /// Finished and saved.
        Completed,
        /// Execution or persistence failed.
        Failed,
        /// Cancellation finished.
        Cancelled,
    }

    /// Bounded process-local status, never a transcript or credential cache.
    #[derive(Debug, Serialize, TS)]
    #[serde(rename_all = "camelCase")]
    #[ts(export)]
    pub struct RunSnapshot {
        /// Service-generated identifier.
        pub run_id: String,
        /// Persisted conversation, if any.
        pub conversation_id: Option<String>,
        /// Persisted candidate, if any.
        pub turn_id: Option<String>,
        /// Current lifecycle state.
        pub status: RunStatus,
        /// Cancellation was requested; not a side-effect rollback guarantee.
        pub cancel_requested: bool,
        /// Safe terminal error.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pub error: Option<KitError>,
        /// Model round limit reached.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pub exhausted: Option<bool>,
    }

    impl Clone for RunSnapshot {
        fn clone(&self) -> Self {
            Self {
                run_id: self.run_id.clone(),
                conversation_id: self.conversation_id.clone(),
                turn_id: self.turn_id.clone(),
                status: self.status,
                cancel_requested: self.cancel_requested,
                error: self.error.as_ref().map(KitError::public_copy),
                exhausted: self.exhausted,
            }
        }
    }
}
