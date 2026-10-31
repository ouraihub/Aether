use super::*;
use aether_chat::Message;
use sha2::{Digest, Sha256};

struct Canonical(Sha256);
impl Canonical {
    fn tag(&mut self, tag: u8) { self.0.update([tag]); }
    fn len(&mut self, len: usize) { self.0.update((len as u64).to_be_bytes()); }
    fn text(&mut self, text: &str) { self.len(text.len()); self.0.update(text.as_bytes()); }
    fn optional(&mut self, value: Option<&str>) {
        match value { Some(value) => { self.tag(1); self.text(value); }, None => self.tag(0) }
    }
    fn visible_text(&mut self, source: &Turn) {
        let texts = || source.messages.iter().filter_map(|message| match message {
            Message::Assistant { text, .. } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        });
        let (length, count) = texts().fold((0_usize, 0_usize), |(length, count), text| {
            (length + text.len(), count + 1)
        });
        self.len(length + count.saturating_sub(1) * 2);
        for (index, text) in texts().enumerate() {
            if index > 0 { self.0.update(b"\n\n"); }
            self.0.update(text.as_bytes());
        }
    }
    fn json(&mut self, value: &serde_json::Value) {
        use serde_json::Value;
        match value {
            Value::Null => self.tag(0),
            Value::Bool(value) => { self.tag(1); self.tag(u8::from(*value)); }
            Value::Number(value) => { self.tag(2); self.text(&value.to_string()); }
            Value::String(value) => { self.tag(3); self.text(value); }
            Value::Array(values) => { self.tag(4); self.len(values.len()); for value in values { self.json(value); } }
            Value::Object(values) => {
                self.tag(5); self.len(values.len());
                let mut keys: Vec<_> = values.keys().collect(); keys.sort_unstable();
                for key in keys { self.text(key); self.json(&values[key]); }
            }
        }
    }
    fn protocol(&mut self, value: aether_support::Protocol) {
        self.tag(match value {
            aether_support::Protocol::AnthropicMessages => 0,
            aether_support::Protocol::OpenaiChat => 1,
            aether_support::Protocol::OpenaiResponses => 2,
        });
    }
    fn effort(&mut self, value: Option<aether_support::ReasoningEffort>) {
        use aether_support::ReasoningEffort;
        self.tag(match value {
            None => 0, Some(ReasoningEffort::Default) => 1,
            Some(ReasoningEffort::None) => 2, Some(ReasoningEffort::Auto) => 3,
            Some(ReasoningEffort::Minimal) => 4, Some(ReasoningEffort::Low) => 5,
            Some(ReasoningEffort::Medium) => 6, Some(ReasoningEffort::High) => 7,
            Some(ReasoningEffort::Xhigh) => 8, Some(ReasoningEffort::Max) => 9,
        });
    }
    fn images(&mut self, turn_id: &str, kind: &str, images: &[super::TurnImage]) {
        self.len(images.len());
        for (idx, image) in images.iter().enumerate() {
            self.text(turn_id);
            self.text(kind);
            self.len(idx);
            self.text(&image.mime);
            match &image.data {
                None => self.tag(0),
                Some(data) => {
                    self.tag(1);
                    self.0.update(Sha256::digest(data.as_bytes()));
                }
            }
        }
    }
    fn message(&mut self, message: &Message) -> Result<(), KitError> {
        match message {
            Message::User(text) => { self.tag(0); self.text(text); }
            Message::UserImages { text, images } => {
                self.tag(1); self.text(text); self.len(images.len());
                for image in images { self.text(&image.mime); self.0.update(Sha256::digest(image.data.as_bytes())); }
            }
            Message::Tool { call_id, text } => { self.tag(2); self.text(call_id); self.text(text); }
            Message::Assistant { text, tool_calls, reasoning } => {
                self.tag(3); self.text(text); self.len(tool_calls.len());
                for call in tool_calls { self.text(&call.id); self.text(&call.name); self.json(&call.args); }
                self.len(reasoning.len());
                for block in reasoning {
                    self.protocol(block.protocol); self.optional(block.endpoint_id.as_deref());
                    self.optional(block.model_id.as_deref()); self.json(&block.value);
                }
            }
        }
        Ok(())
    }
    fn status(&mut self, status: TurnStatus) {
        self.tag(match status {
            TurnStatus::Running => 0, TurnStatus::Completed => 1,
            TurnStatus::Failed => 2, TurnStatus::Cancelled => 3,
        });
    }
}

pub(super) fn fingerprint(action: &ActionRequest, conversation: Option<&Conversation>, input: &ReservationInput) -> Result<PreparationFingerprint, KitError> {
    let mut hash = Canonical(Sha256::new());
    hash.text("aether.conversation.preparation.v1");
    hash.optional(action.conversation_id.as_deref());
    hash.optional(conversation.and_then(|c| c.summary.scope.as_deref()).or_else(|| conversation.is_none().then(|| action.scope.as_deref()).flatten()));
    match &action.action {
        ResolvedAction::Reply { parent_id } => { hash.tag(0); hash.optional(parent_id.as_deref()); }
        ResolvedAction::Retry { source_id } => { hash.tag(1); hash.text(source_id); }
        ResolvedAction::Translate { source_id, target_language } => { hash.tag(2); hash.text(source_id); hash.text(target_language); }
    }
    match &action.selection_guard {
        SelectionGuard::Explicit => hash.tag(0),
        SelectionGuard::Selected { selected_turn_id } => { hash.tag(1); hash.text(selected_turn_id); }
        SelectionGuard::LatestCompleted { head_id } => { hash.tag(2); hash.optional(head_id.as_deref()); }
    }
    if let ResolvedAction::Retry { source_id } | ResolvedAction::Translate { source_id, .. } = &action.action {
        let source = target_turn(conversation.ok_or_else(|| KitError::not_found("conversation"))?, source_id)?;
        hash.text(&source.id); hash.status(source.status); hash.optional(source.parent_id.as_deref());
        hash.text(&source.question_id); hash.optional(source.supersedes_id.as_deref()); hash.text(&source.kind);
        hash.optional(source.endpoint_id.as_deref()); hash.optional(source.model_id.as_deref()); hash.text(&source.input);
        if matches!(&action.action, ResolvedAction::Translate { .. }) { hash.visible_text(source); }
        hash.images(&source.id, "input", &source.images); hash.images(&source.id, "generated", &source.generated);
    }
    hash.text(&input.input); hash.len(input.images.len());
    for (idx, image) in input.images.iter().enumerate() { hash.len(idx); hash.text(&image.mime); hash.0.update(Sha256::digest(image.data.as_bytes())); }
    hash.len(input.history_path.len());
    for turn in &input.history_path {
        hash.text(&turn.id); hash.optional(turn.parent_id.as_deref()); hash.status(turn.status); hash.text(&turn.input);
        hash.len(turn.messages.len()); for message in &turn.messages { hash.message(message)?; }
        hash.optional(turn.endpoint_id.as_deref()); hash.optional(turn.model_id.as_deref()); hash.effort(turn.reasoning_effort);
        hash.images(&turn.id, "input", &turn.images); hash.images(&turn.id, "generated", &turn.generated);
    }
    Ok(PreparationFingerprint(hash.0.finalize().into()))
}

pub(super) fn preparation_error(error: KitError) -> KitError {
    if matches!(error.code(), aether_support::KitErrorCode::NotFound | aether_support::KitErrorCode::Conflict | aether_support::KitErrorCode::BadRequest) { KitError::conflict("preparation_changed") } else { error }
}
