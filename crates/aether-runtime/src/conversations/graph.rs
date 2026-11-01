//! Pure ancestry validation, selection repair and visible-reply translation.
use aether_chat::Message;
use aether_support::KitError;

use super::{Conversation, Turn};

/// Resolves one ancestor chain, rejecting missing, cyclic and foreign nodes.
/// # Errors
/// Missing node, cyclic ancestry or a parent from another conversation.
pub fn history_path(turns: &[Turn], head: Option<&str>) -> Result<Vec<Turn>, KitError> {
    let by_id = turns
        .iter()
        .map(|t| (t.id.as_str(), t))
        .collect::<std::collections::HashMap<_, _>>();
    let owner = head
        .and_then(|id| by_id.get(id))
        .map(|t| t.conversation_id.as_str());
    let mut seen = std::collections::HashSet::new();
    let mut path = Vec::new();
    let mut current = head;
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(KitError::conflict("history_cycle"));
        }
        let turn = by_id.get(id).ok_or_else(|| KitError::not_found("turn"))?;
        if Some(turn.conversation_id.as_str()) != owner {
            return Err(KitError::conflict("history_foreign_turn"));
        }
        current = turn.parent_id.as_deref();
        path.push((*turn).clone());
    }
    path.reverse();
    Ok(path)
}

/// Creates the fixed instruction from visible assistant text, excluding tool/reasoning output.
/// # Errors
/// Empty visible reply or invalid target language.
pub fn translation_input(turn: &Turn, language: &str) -> Result<String, KitError> {
    let text = turn
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant { text, .. } if !text.is_empty() => Some(text.as_str()),
            Message::User(_)
            | Message::UserImages { .. }
            | Message::Assistant { .. }
            | Message::Tool { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    translation_input_text(&text, language)
}

fn translation_input_text(text: &str, language: &str) -> Result<String, KitError> {
    if text.trim().is_empty() {
        return Err(KitError::bad_request("translateTurnId", "empty_reply"));
    }
    let language = language.trim();
    if language.is_empty() || language.chars().count() > 80 {
        return Err(KitError::bad_request("targetLanguage", "invalid"));
    }
    Ok(format!(
        "Translate the following reply into {language}. Return only the translation, preserving Markdown formatting.\n\n{text}"
    ))
}

pub(super) fn target_turn<'a>(c: &'a Conversation, id: &str) -> Result<&'a Turn, KitError> {
    let mut seen = std::collections::HashSet::new();
    let mut current = Some(id);
    while let Some(node) = current {
        if !seen.insert(node) {
            return Err(KitError::conflict("history_cycle"));
        }
        let turn = c
            .turns
            .iter()
            .find(|turn| turn.id == node)
            .ok_or_else(|| KitError::not_found("turn"))?;
        if turn.conversation_id != c.summary.id {
            return Err(KitError::conflict("history_foreign_turn"));
        }
        current = turn.parent_id.as_deref();
    }
    c.turns
        .iter()
        .find(|t| t.id == id && t.conversation_id == c.summary.id)
        .ok_or_else(|| KitError::not_found("turn"))
}

/// Newest reply leaf below `start` (or below the newest root reply when `None`).
pub(super) fn latest_leaf(c: &Conversation, start: Option<String>) -> Option<String> {
    let newest_child = |parent: Option<&str>| {
        c.turns
            .iter()
            .filter(|t| t.kind == "reply" && t.parent_id.as_deref() == parent)
            .max_by_key(|t| t.seq)
            .map(|t| t.id.clone())
    };
    let mut current = start.or_else(|| newest_child(None))?;
    while let Some(next) = newest_child(Some(&current)) {
        current = next;
    }
    Some(current)
}

pub(super) fn ensure_leaf(c: &Conversation, id: &str) -> Result<(), KitError> {
    if c.turns
        .iter()
        .any(|t| t.parent_id.as_deref() == Some(id) || t.supersedes_id.as_deref() == Some(id))
    {
        return Err(KitError::conflict("turn_has_dependents"));
    }
    Ok(())
}

pub(super) fn visible_text(turn: &Turn) -> String {
    turn.messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant { text, .. } if !text.is_empty() => Some(text.as_str()),
            Message::User(_)
            | Message::UserImages { .. }
            | Message::Assistant { .. }
            | Message::Tool { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
