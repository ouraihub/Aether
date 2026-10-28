//! Pure history budgeting, consumed by runtime preparation.
use crate::conversations::{Turn, TurnStatus};
use aether_chat::Message;

/// Keeps complete completed turns, dropping the oldest until the character budget fits.
#[must_use]
pub(crate) fn trim_history(turns: &[Turn], max_tokens: u32) -> Vec<Message> {
    let limit = usize::try_from(max_tokens).unwrap_or(usize::MAX);
    let mut chars = 0_usize;
    let mut start = turns.len();
    for (index, turn) in turns.iter().enumerate().rev() {
        if turn.status != TurnStatus::Completed {
            continue;
        }
        chars = chars
            .saturating_add(turn.input.chars().count())
            .saturating_add(turn.messages.iter().map(message_chars).sum::<usize>());
        if chars.div_ceil(3) > limit {
            break;
        }
        start = index;
    }
    turns[start..]
        .iter()
        .filter(|turn| turn.status == TurnStatus::Completed)
        .flat_map(|turn| {
            let images = turn.image_parts();
            let text = turn.input.clone();
            let user = if images.is_empty() {
                Message::User(text)
            } else {
                Message::UserImages { text, images }
            };
            std::iter::once(user).chain(turn.messages.iter().cloned())
        })
        .collect()
}

pub(crate) fn message_chars(message: &Message) -> usize {
    match message {
        Message::User(text) | Message::UserImages { text, .. } | Message::Tool { text, .. } => {
            text.chars().count()
        }
        Message::Assistant {
            text,
            tool_calls,
            reasoning,
        } => {
            text.chars().count()
                + reasoning
                    .iter()
                    .map(|block| block.value.to_string().chars().count())
                    .sum::<usize>()
                + tool_calls
                    .iter()
                    .map(|call| call.name.chars().count() + call.args.to_string().chars().count())
                    .sum::<usize>()
        }
    }
}
