//! Read-only action normalization and authoritative reservation input assembly.
use super::fingerprint::fingerprint;
use super::graph::target_turn;
use super::{
    ActionRequest, Conversation, PreparationFingerprint, ReservationInput, ResolvedAction,
    SelectionGuard, SourceModelRef, TurnStatus, history_path, translation_input,
};
use aether_images::ImagePart;
use aether_support::KitError;

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

pub(super) fn preparation_error(error: KitError) -> KitError {
    if matches!(
        error.code(),
        aether_support::KitErrorCode::NotFound
            | aether_support::KitErrorCode::Conflict
            | aether_support::KitErrorCode::BadRequest
    ) {
        KitError::conflict("preparation_changed")
    } else {
        error
    }
}
