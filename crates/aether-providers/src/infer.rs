//! Model metadata inferred from the id alone (docs/11 §6). Test data:
//! `fixtures/model-classify.json`, shared with the TypeScript side.

use crate::ModelType;

/// What [`infer`] can tell from a model id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inferred {
    /// Model type by keyword.
    pub model_type: ModelType,
    /// Context window of the family, when known.
    pub context_window: Option<u32>,
    /// Maximum output of the family, when known.
    pub max_output: Option<u32>,
    /// Whether the family accepts images.
    pub can_vision: bool,
    /// `Some(true)` for chat models of a known family, otherwise unknown.
    pub supports_tools: Option<bool>,
    /// Family name for grouping, e.g. `claude`.
    pub group: Option<String>,
}

/// Type keywords, checked in order; the first hit wins.
const TYPES: &[(&[&str], ModelType)] = &[
    (&["embed"], ModelType::Embedding),
    (&["rerank"], ModelType::Rerank),
    (&["tts", "speech"], ModelType::Tts),
    (&["whisper", "stt", "transcri"], ModelType::Stt),
    (&["dall-e", "image", "flux", "imagen"], ModelType::Image),
    (&["video", "sora", "veo"], ModelType::Video),
];

/// Family limits: (context window, max output, vision).
type Limits = (u32, u32, bool);

/// Known variants before their family default; the first prefix match wins.
const FAMILIES: &[(&str, &str, Limits)] = &[
    ("claude", "claude", (200_000, 8_192, true)),
    ("gpt-3.5", "gpt", (16_385, 4_096, false)),
    ("gpt", "gpt", (128_000, 16_384, true)),
    ("gemini", "gemini", (1_048_576, 8_192, true)),
    ("deepseek", "deepseek", (128_000, 8_192, false)),
    ("qwen", "qwen", (131_072, 8_192, false)),
    ("glm", "glm", (128_000, 4_096, false)),
    ("kimi", "kimi", (131_072, 8_192, false)),
    ("moonshot-v1-8k", "moonshot", (8_192, 4_096, false)),
    ("moonshot-v1-32k", "moonshot", (32_768, 4_096, false)),
    ("moonshot", "moonshot", (131_072, 4_096, false)),
];

/// OpenAI reasoning models `o<digit>…`; not a plain prefix (`omni-…` is not one).
const O_SERIES: Limits = (200_000, 100_000, true);

/// Infers type, limits and group from `model_id` (case-insensitive; a vendor path
/// such as `Qwen/…` is ignored for the family).
// ponytail: static family table, wrong for new variants until the table is updated.
#[must_use]
pub fn infer(model_id: &str) -> Inferred {
    let id = model_id.to_ascii_lowercase();
    let model_type = TYPES
        .iter()
        .find(|(words, _)| words.iter().any(|w| id.contains(w)))
        .map_or(ModelType::Chat, |&(_, t)| t);
    let name = id.rsplit('/').next().unwrap_or_default();
    let family = FAMILIES
        .iter()
        .find(|(prefix, ..)| name.starts_with(prefix))
        .map(|&(_, group, limits)| (group, limits))
        .or_else(|| {
            let mut chars = name.chars();
            (chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit()))
                .then_some(("o", O_SERIES))
        });
    let group = family.map(|(group, _)| group.to_owned());
    match family {
        Some((_, (context, output, vision))) if model_type == ModelType::Chat => Inferred {
            model_type,
            context_window: Some(context),
            max_output: Some(output),
            can_vision: vision,
            supports_tools: Some(true),
            group,
        },
        // Limits describe the family's chat models; other types only keep the group.
        _ => Inferred {
            model_type,
            context_window: None,
            max_output: None,
            can_vision: false,
            supports_tools: None,
            group,
        },
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Case {
        id: String,
        model_type: ModelType,
        context_window: Option<u32>,
        max_output: Option<u32>,
        can_vision: bool,
        supports_tools: Option<bool>,
        group: Option<String>,
    }

    #[test]
    fn every_fixture_entry_matches() {
        let text = include_str!("../../../fixtures/model-classify.json");
        let cases: Vec<Case> = serde_json::from_str(text).unwrap();
        assert!(cases.len() >= 30);
        for case in cases {
            let expected = Inferred {
                model_type: case.model_type,
                context_window: case.context_window,
                max_output: case.max_output,
                can_vision: case.can_vision,
                supports_tools: case.supports_tools,
                group: case.group,
            };
            assert_eq!(infer(&case.id), expected, "{}", case.id);
        }
    }
}
