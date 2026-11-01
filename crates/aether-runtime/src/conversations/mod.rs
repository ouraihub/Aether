//! Conversation domain, store contract and model-independent service (docs/26).
//!
//! The root owns the public contract; `action` resolves reservation inputs and
//! `fingerprint` hashes consumed preparation fields. `graph` owns pure history
//! operations; `sqlite` owns transactions, and `rows` decodes stored records.
use std::sync::Arc;

use aether_chat::{EndStatus, Message, RunInput};
use aether_images::ImagePart;
use aether_support::{KitError, SkillRef};

mod action;
mod fingerprint;
mod graph;
mod rows;
mod sqlite;
pub use action::resolve_action;
pub use graph::{history_path, translation_input};
pub use sqlite::SqliteStore;

#[cfg(test)]
use sqlite::MIGRATIONS;

/// Stored image retention, in days.
pub const IMAGE_TTL_DAYS: i64 = 7;

/// A fixed action and its preparation basis, checked again during reservation.
#[derive(Debug, Clone)]
pub struct ActionRequest {
    /// Existing conversation, or none to create one on successful reservation.
    pub conversation_id: Option<String>,
    /// Host-defined scope for a new conversation.
    pub scope: Option<String>,
    /// Original client text; sourced actions derive their effective text instead.
    pub input: String,
    /// Original client images, allowed only on ordinary replies.
    pub images: Vec<ImagePart>,
    /// Concrete source or parent chosen before host preparation.
    pub action: ResolvedAction,
    /// Selection state actually consumed when choosing the source or parent.
    pub selection_guard: SelectionGuard,
    /// Digest of the consumed conversation fields and image contents.
    pub expected: PreparationFingerprint,
}
/// A normalized action that never reselects a source inside the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAction {
    /// An ordinary reply below the fixed completed parent.
    Reply {
        /// Completed context head, or none for empty history.
        parent_id: Option<String>,
    },
    /// A new candidate retaining its source input, branch and question.
    Retry {
        /// Existing nonrunning source candidate.
        source_id: String,
    },
    /// A translation below the source, using only visible assistant text.
    Translate {
        /// Existing nonrunning answer to translate.
        source_id: String,
        /// Trimmed target language, between one and eighty characters.
        target_language: String,
    },
}
/// The implicit selection dependency, if any, consumed during action resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionGuard {
    /// The request named its source or parent explicitly.
    Explicit,
    /// The persisted selection supplied the source or parent.
    Selected {
        /// Selection that must still be persisted at reservation.
        selected_turn_id: String,
    },
    /// No selection existed, so the latest completed turn supplied the head.
    LatestCompleted {
        /// Latest completed head, including none for an empty conversation.
        head_id: Option<String>,
    },
}
/// Versioned canonical SHA-256 digest of preparation-consumed fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationFingerprint([u8; 32]);
/// Authoritative effective input and selected path supplied to pure validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ReservationInput {
    /// Effective text, derived from the source for retry and translation.
    pub input: String,
    /// Current effective images whose bytes remain available.
    pub images: Vec<ImagePart>,
    /// Exact ancestor path, in root-to-head order, including image references.
    pub history_path: Vec<Turn>,
    /// Stored source model when both endpoint and model identities are available.
    pub source_model: Option<SourceModelRef>,
}
/// Pure stored model identity, independent of the providers crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceModelRef {
    /// Endpoint that produced the source answer.
    pub endpoint_id: String,
    /// Model that produced the source answer.
    pub model_id: String,
}
/// A committed running candidate and the exact execution input validated for it.
pub struct TurnReservation {
    /// Fixed persisted identity and branch metadata.
    pub handle: TurnHandle,
    /// Authoritative input used by the validation callback.
    pub input: ReservationInput,
    /// Final execution input returned directly by the callback.
    pub run_input: RunInput,
}

impl std::fmt::Debug for TurnReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnReservation")
            .finish_non_exhaustive()
    }
}

/// Model-independent conversation use cases. Async callers use a blocking thread.
#[derive(Clone)]
pub struct ConversationService {
    store: Arc<dyn ConversationStore>,
}

impl ConversationService {
    /// Wraps a shared store without starting tasks or resolving a model.
    pub fn new(store: Arc<dyn ConversationStore>) -> Self {
        Self { store }
    }
    /// Lists at most two hundred conversations, most recently updated first.
    /// # Errors
    /// Invalid scope length or an underlying storage failure.
    pub fn list(&self, scope: Option<&str>) -> Result<Vec<ConversationSummary>, KitError> {
        if scope.is_some_and(|scope| scope.chars().count() > 200) {
            return Err(KitError::bad_request("scope", "too_long"));
        }
        self.store.list(scope, 200)
    }
    /// Loads one conversation with all candidates in sequence order.
    /// # Errors
    /// Invalid identity, missing conversation, or storage failure.
    pub fn get(&self, id: &str) -> Result<Conversation, KitError> {
        check_id("id", id)?;
        self.store.load(id)
    }
    /// Trims and replaces a conversation title without changing its update time.
    /// # Errors
    /// Invalid identity/title, missing conversation, or storage failure.
    pub fn rename(&self, id: &str, title: &str) -> Result<ConversationSummary, KitError> {
        check_id("id", id)?;
        if title.chars().count() > 200 {
            return Err(KitError::bad_request("title", "too_long"));
        }
        let title = title.trim();
        if title.is_empty() {
            return Err(KitError::bad_request("title", "empty"));
        }
        self.store.rename(id, title)
    }
    /// Replaces a terminal leaf's visible text using the supplied expected text.
    /// # Errors
    /// Invalid identity/text, missing turn, running/dependent turn, stale expected
    /// text, or storage failure.
    pub fn edit_turn(
        &self,
        conversation_id: &str,
        turn_id: &str,
        text: &str,
        expected_text: &str,
    ) -> Result<Conversation, KitError> {
        check_id("conversationId", conversation_id)?;
        check_id("turnId", turn_id)?;
        if text.chars().count() > 24_000 {
            return Err(KitError::bad_request("text", "too_long"));
        }
        if text.trim().is_empty() {
            return Err(KitError::bad_request("text", "empty"));
        }
        self.store
            .edit_turn(conversation_id, turn_id, text, expected_text)
    }
    /// Removes a candidate and direct translations, repairing graph successors.
    /// # Errors
    /// Invalid identity, missing turn, running conversation, or storage failure.
    pub fn remove_turn(
        &self,
        conversation_id: &str,
        turn_id: &str,
    ) -> Result<Conversation, KitError> {
        check_id("conversationId", conversation_id)?;
        check_id("turnId", turn_id)?;
        self.store.remove_turn(conversation_id, turn_id)
    }
    /// Copies only the requested ancestor path into a new conversation.
    /// # Errors
    /// Invalid identity/history, missing turn, running conversation, or storage failure.
    pub fn fork(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError> {
        check_id("conversationId", conversation_id)?;
        check_id("turnId", turn_id)?;
        self.store.fork(conversation_id, turn_id)
    }
    /// Persists a completed candidate as the next reply's selected context head.
    /// # Errors
    /// Invalid identity, missing/noncompleted turn, running conversation, or storage failure.
    pub fn select_turn(
        &self,
        conversation_id: &str,
        turn_id: &str,
    ) -> Result<Conversation, KitError> {
        check_id("conversationId", conversation_id)?;
        check_id("turnId", turn_id)?;
        self.store.select_turn(conversation_id, turn_id)
    }
    /// Deletes a whole conversation and its turns and images.
    /// # Errors
    /// Invalid identity, missing conversation, or storage failure.
    pub fn delete(&self, id: &str) -> Result<(), KitError> {
        check_id("id", id)?;
        self.store.delete(id)
    }
    /// Persists generated images atomically; storage failures remain visible to the tool.
    /// # Errors
    /// Invalid identity, missing turn, or storage failure.
    pub fn add_generated(&self, turn_id: &str, images: &[ImagePart]) -> Result<(), KitError> {
        check_id("turnId", turn_id)?;
        self.store.add_generated(turn_id, images)
    }
}

fn check_id(field: &str, id: &str) -> Result<(), KitError> {
    if id.trim().is_empty() || id.chars().count() > 128 {
        return Err(KitError::bad_request(field, "invalid"));
    }
    Ok(())
}

/// Where conversations live. Implemented by [`SqliteStore`] and by hosts that keep
/// conversations in their own database.
pub trait ConversationStore: Send + Sync {
    /// Most recently updated first; `scope: None` lists every scope.
    ///
    /// # Errors
    /// `internal` on storage failure.
    fn list(&self, scope: Option<&str>, limit: usize)
    -> Result<Vec<ConversationSummary>, KitError>;
    /// One conversation with all its turns in order.
    ///
    /// # Errors
    /// `not_found{entity:"conversation"}`; `internal`.
    fn load(&self, id: &str) -> Result<Conversation, KitError>;
    /// Rechecks consumed preparation fields and validates before inserting a running turn.
    /// The callback must perform only pure input assembly and captured atomic
    /// closing checks; it must not perform I/O or reenter the store.
    /// # Errors
    /// `conflict` when preparation changed or another turn is running; callback
    /// validation errors propagate unchanged. Any error rolls back all inserts
    /// and metadata changes; storage failures remain `internal`.
    fn reserve_action(
        &self,
        action: &ActionRequest,
        validate: &mut dyn FnMut(&ReservationInput) -> Result<aether_chat::RunInput, KitError>,
    ) -> Result<TurnReservation, KitError>;
    /// Writes messages and the terminal status atomically.
    /// # Errors
    /// Missing/nonrunning turn or storage failure.
    fn finish_turn(&self, turn: TurnHandle, record: TurnRecord) -> Result<(), KitError>;
    /// Deletes a conversation and all its candidates.
    /// # Errors
    /// Missing conversation or storage failure.
    fn delete(&self, id: &str) -> Result<(), KitError>;
    /// Trims and updates the title.
    /// # Errors
    /// Empty title, missing conversation or storage failure.
    fn rename(&self, id: &str, title: &str) -> Result<ConversationSummary, KitError>;
    /// Replaces a terminal leaf's visible assistant text using compare-and-swap.
    /// # Errors
    /// Invalid text, stale expected text, running/dependent turn or storage failure.
    fn edit_turn(
        &self,
        conversation_id: &str,
        turn_id: &str,
        text: &str,
        expected_text: &str,
    ) -> Result<Conversation, KitError>;
    /// Deletes a turn and its translations, reparenting dependents onto the successor
    /// (next/previous candidate, else parent) and moving a deleted selection to the
    /// successor's newest reply leaf (docs/35 §3.3).
    /// # Errors
    /// Missing turn, running conversation or storage failure.
    fn remove_turn(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError>;
    /// Copies exactly the target's ancestor path into an independent conversation.
    /// # Errors
    /// Missing/invalid history, running conversation or storage failure.
    fn fork(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError>;
    /// Persists a completed context head without cloning the conversation.
    /// # Errors
    /// Missing/noncompleted turn, running conversation or storage failure.
    fn select_turn(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError>;
    /// Clears the bytes of images created before `before_ms`, keeping their rows so they
    /// show as expired; returns how many were cleared.
    /// # Errors
    /// `internal` on storage failure.
    fn purge_images(&self, before_ms: i64) -> Result<usize, KitError>;
    /// Appends images produced by a tool (`generate_image`) during `turn_id`'s run (docs/20 §3).
    /// # Errors
    /// `internal` on storage failure.
    fn add_generated(&self, turn_id: &str, images: &[ImagePart]) -> Result<(), KitError>;
}

/// A row of the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    /// Conversation id.
    pub id: String,
    /// Title.
    pub title: String,
    /// Host-defined scope.
    pub scope: Option<String>,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds.
    pub updated_at: i64,
    /// Completed context head used for subsequent normal input.
    pub selected_turn_id: Option<String>,
}

/// A conversation with its turns, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    /// Metadata.
    pub summary: ConversationSummary,
    /// Turns ordered by `seq`.
    pub turns: Vec<Turn>,
}

/// State of a stored turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    /// Started and not finished (or regenerating).
    Running,
    /// Finished normally; the only status that enters later history.
    Completed,
    /// Failed; `error_code` says why (`interrupted` after a crash).
    Failed,
    /// Stopped by the user.
    Cancelled,
}

/// User input plus every assistant/tool message the run produced for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// Turn id.
    pub id: String,
    /// 1-based position in the conversation.
    pub seq: u32,
    /// State.
    pub status: TurnStatus,
    /// User input.
    pub input: String,
    /// Messages of the run (kept for failed/cancelled turns too, for display).
    pub messages: Vec<Message>,
    /// Model used.
    pub model_id: Option<String>,
    /// Skills used (id + revision, no body).
    pub skills: Vec<SkillRef>,
    /// Error code of a failed turn.
    pub error_code: Option<String>,
    /// Input tokens reported by the upstream.
    pub input_tokens: Option<u32>,
    /// Output tokens reported by the upstream.
    pub output_tokens: Option<u32>,
    /// Tokens read from provider cache.
    pub cache_read_tokens: Option<u32>,
    /// Tokens written to provider cache.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning output tokens.
    pub reasoning_tokens: Option<u32>,
    /// Last request context report or estimate.
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Requested reasoning effort.
    pub reasoning_effort: Option<aether_support::ReasoningEffort>,
    /// Endpoint used for this turn.
    pub endpoint_id: Option<String>,
    /// Cost fixed when the turn finished; never recomputed (docs/41 §4).
    pub cost: Option<aether_support::Cost>,
    /// Conversation owning the graph node.
    pub conversation_id: String,
    /// Previous context head.
    pub parent_id: Option<String>,
    /// Question shared by alternative candidates.
    pub question_id: String,
    /// Source candidate retried by this turn.
    pub supersedes_id: Option<String>,
    /// `reply` or `translation`.
    pub kind: String,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Input images in order; `data: None` once purged.
    pub images: Vec<TurnImage>,
    /// Images a tool generated during this turn; `data: None` once purged. Never replayed.
    pub generated: Vec<TurnImage>,
}

impl Turn {
    /// Images whose bytes are still stored, in order; expired ones are skipped.
    #[must_use]
    pub fn image_parts(&self) -> Vec<ImagePart> {
        self.images
            .iter()
            .filter_map(|image| {
                Some(ImagePart {
                    mime: image.mime.clone(),
                    data: image.data.clone()?,
                })
            })
            .collect()
    }
}

/// A stored input image; `data` (base64) is `None` after [`ConversationStore::purge_images`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnImage {
    /// MIME type.
    pub mime: String,
    /// Base64 bytes.
    pub data: Option<String>,
}

/// A turn being run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnHandle {
    /// Conversation id.
    pub conversation_id: String,
    /// Turn id.
    pub turn_id: String,
    /// Canonical input used for this run.
    pub input: String,
    /// Reserved parent context head.
    pub parent_id: Option<String>,
    /// Reserved candidate group.
    pub question_id: String,
    /// Source candidate for a retry.
    pub supersedes_id: Option<String>,
    /// `reply` or `translation`.
    pub kind: String,
}

/// Result of a turn, written by [`ConversationStore::finish_turn`].
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRecord {
    /// Terminal status.
    pub status: EndStatus,
    /// Messages of the run, including partial text of a failed/cancelled run.
    pub messages: Vec<Message>,
    /// Model used.
    pub model_id: Option<String>,
    /// Skills used.
    pub skills: Vec<SkillRef>,
    /// Error code when `status` is `failed`.
    pub error_code: Option<String>,
    /// Input tokens.
    pub input_tokens: Option<u32>,
    /// Output tokens.
    pub output_tokens: Option<u32>,
    /// Tokens read from provider cache.
    pub cache_read_tokens: Option<u32>,
    /// Tokens written to provider cache.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning output tokens.
    pub reasoning_tokens: Option<u32>,
    /// Last request context report or estimate.
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Requested reasoning effort.
    pub reasoning_effort: Option<aether_support::ReasoningEffort>,
    /// Endpoint used for this turn.
    pub endpoint_id: Option<String>,
    /// Turn cost from the model's pricing.
    pub cost: Option<aether_support::Cost>,
}

#[cfg(test)]
mod tests;
