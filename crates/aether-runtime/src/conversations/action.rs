//! Read-only action resolution and consumed-field preparation fingerprints.
use super::{
    Conversation, Turn, TurnHandle, TurnStatus, history_path, target_turn, translation_input,
};
use aether_chat::{Message, RunInput};
use aether_images::ImagePart;
use aether_support::KitError;
use sha2::{Digest, Sha256};

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

/// Normalizes once before host/provider/tool preparation; never mutates the store.
///
/// # Errors
/// Returns `bad_request` for invalid fields or conflicting actions, `not_found`
/// for missing conversation/source nodes, and `conflict` for running or invalid
/// history/selection state. Storage loading is performed by the caller.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the public request without depending on runs"
)]
pub fn resolve_action(
    conversation: Option<&Conversation>,
    conversation_id: Option<String>,
    scope: Option<String>,
    input: String,
    images: Vec<ImagePart>,
    regenerate: bool,
    reply_to_id: Option<String>,
    retry_turn_id: Option<String>,
    translate_turn_id: Option<String>,
    target_language: Option<String>,
) -> Result<(ActionRequest, ReservationInput), KitError> {
    for (field, id) in [
        ("conversationId", &conversation_id),
        ("replyToId", &reply_to_id),
        ("retryTurnId", &retry_turn_id),
        ("translateTurnId", &translate_turn_id),
    ] {
        if id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || id.chars().count() > 128)
        {
            return Err(KitError::bad_request(field, "invalid"));
        }
    }
    if scope
        .as_ref()
        .is_some_and(|scope| scope.chars().count() > 200)
    {
        return Err(KitError::bad_request("scope", "too_long"));
    }
    if input.chars().count() > 100_000 {
        return Err(KitError::bad_request("input", "too_long"));
    }
    let sourced = retry_turn_id.is_some() || translate_turn_id.is_some() || regenerate;
    if usize::from(retry_turn_id.is_some())
        + usize::from(translate_turn_id.is_some())
        + usize::from(regenerate)
        > 1
    {
        return Err(KitError::bad_request("action", "multiple"));
    }
    if reply_to_id.is_some() && sourced {
        return Err(KitError::bad_request("replyToId", "action_conflict"));
    }
    if sourced && !images.is_empty() {
        return Err(KitError::bad_request("images", "action_conflict"));
    }
    if translate_turn_id.is_some() != target_language.is_some() {
        return Err(KitError::bad_request(
            "targetLanguage",
            "translation_required",
        ));
    }
    if conversation_id.is_none() && (sourced || reply_to_id.is_some()) {
        return Err(KitError::bad_request("conversationId", "required"));
    }
    if conversation_id.as_deref() != conversation.map(|c| c.summary.id.as_str()) {
        return Err(KitError::not_found("conversation"));
    }
    if conversation.is_some_and(|c| c.turns.iter().any(|t| t.status == TurnStatus::Running)) {
        return Err(KitError::conflict("turn_running"));
    }
    let (head, guard) = if let Some(id) = reply_to_id {
        (Some(id), SelectionGuard::Explicit)
    } else if retry_turn_id.is_some() || translate_turn_id.is_some() || conversation.is_none() {
        (None, SelectionGuard::Explicit)
    } else if let Some(id) = conversation.and_then(|c| c.summary.selected_turn_id.clone()) {
        (
            Some(id.clone()),
            SelectionGuard::Selected {
                selected_turn_id: id,
            },
        )
    } else {
        let head_id = conversation.and_then(latest_completed).map(str::to_owned);
        (head_id.clone(), SelectionGuard::LatestCompleted { head_id })
    };
    let action = if let Some(source_id) = translate_turn_id {
        let language = target_language.unwrap_or_default();
        let language = language.trim();
        if language.is_empty() || language.chars().count() > 80 {
            return Err(KitError::bad_request("targetLanguage", "invalid"));
        }
        ResolvedAction::Translate {
            source_id,
            target_language: language.to_owned(),
        }
    } else if let Some(source_id) = retry_turn_id {
        ResolvedAction::Retry { source_id }
    } else if regenerate {
        ResolvedAction::Retry {
            source_id: head.ok_or_else(|| KitError::not_found("turn"))?,
        }
    } else {
        ResolvedAction::Reply { parent_id: head }
    };
    let mut request = ActionRequest {
        conversation_id,
        scope,
        input,
        images,
        action,
        selection_guard: guard,
        expected: PreparationFingerprint([0; 32]),
    };
    let reservation = reservation_input(&request, conversation)?;
    request.expected = fingerprint(&request, conversation, &reservation)?;
    Ok((request, reservation))
}

fn latest_completed(c: &Conversation) -> Option<&str> {
    c.turns
        .iter()
        .filter(|t| t.status == TurnStatus::Completed)
        .max_by_key(|t| t.seq)
        .map(|t| t.id.as_str())
}

pub(super) fn reservation_input(
    action: &ActionRequest,
    conversation: Option<&Conversation>,
) -> Result<ReservationInput, KitError> {
    if action.conversation_id.as_deref() != conversation.map(|c| c.summary.id.as_str()) {
        return Err(KitError::not_found("conversation"));
    }
    match &action.selection_guard {
        SelectionGuard::Explicit => {}
        SelectionGuard::Selected { selected_turn_id }
            if conversation.and_then(|c| c.summary.selected_turn_id.as_deref())
                == Some(selected_turn_id.as_str()) => {}
        SelectionGuard::LatestCompleted { head_id }
            if conversation.is_some_and(|c| {
                c.summary.selected_turn_id.is_none() && latest_completed(c) == head_id.as_deref()
            }) => {}
        SelectionGuard::Selected { .. } | SelectionGuard::LatestCompleted { .. } => {
            return Err(KitError::conflict("preparation_changed"));
        }
    }
    let (input, images, head, source_model) = match &action.action {
        ResolvedAction::Reply { parent_id } => {
            if let Some(id) = parent_id {
                let c = conversation.ok_or_else(|| KitError::not_found("conversation"))?;
                if target_turn(c, id)?.status != TurnStatus::Completed {
                    return Err(KitError::conflict("turn_not_completed"));
                }
            }
            (
                action.input.clone(),
                action.images.clone(),
                parent_id.as_deref(),
                None,
            )
        }
        ResolvedAction::Retry { source_id } | ResolvedAction::Translate { source_id, .. } => {
            if !action.images.is_empty() {
                return Err(KitError::bad_request("images", "action_conflict"));
            }
            let c = conversation.ok_or_else(|| KitError::not_found("conversation"))?;
            let source = target_turn(c, source_id)?;
            if source.status == TurnStatus::Running {
                return Err(KitError::conflict("turn_running"));
            }
            let model = source
                .endpoint_id
                .as_ref()
                .zip(source.model_id.as_ref())
                .map(|(endpoint_id, model_id)| SourceModelRef {
                    endpoint_id: endpoint_id.clone(),
                    model_id: model_id.clone(),
                });
            match &action.action {
                ResolvedAction::Translate {
                    target_language, ..
                } => (
                    translation_input(source, target_language)?,
                    vec![],
                    Some(source.id.as_str()),
                    model,
                ),
                ResolvedAction::Reply { .. } | ResolvedAction::Retry { .. } => (
                    source.input.clone(),
                    source.image_parts(),
                    source.parent_id.as_deref(),
                    model,
                ),
            }
        }
    };
    if input.chars().count() > 100_000 {
        return Err(KitError::bad_request("input", "too_long"));
    }
    let path = match conversation {
        Some(c) => history_path(&c.turns, head)?,
        None => vec![],
    };
    Ok(ReservationInput {
        input,
        images,
        history_path: path,
        source_model,
    })
}

#[path = "fingerprint.rs"]
mod fingerprint;
pub(super) use fingerprint::{fingerprint, preparation_error};
