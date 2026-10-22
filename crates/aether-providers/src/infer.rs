//! Model metadata inferred from the id alone (docs/11 §6). Test data:
//! `packages/ui/fixtures/model-classify.json`, shared with the TypeScript side.

use crate::{ModelType, Protocol, ReasoningConfig, ReasoningDialect, ReasoningEffort};

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

/// Image-input markers in a model name; they override a family's non-vision default
/// (`qwen-vl-max`, `glm-4v-plus`, `llama-3.2-…-vision`).
const VISION_MARKERS: &[&str] = &["vision", "-vl", "vl-", "4v"];

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
    let marked = VISION_MARKERS.iter().any(|m| name.contains(m));
    match family {
        Some((_, (context, output, vision))) if model_type == ModelType::Chat => Inferred {
            model_type,
            context_window: Some(context),
            max_output: Some(output),
            can_vision: vision || marked,
            supports_tools: Some(true),
            group,
        },
        // Limits describe the family's chat models; other types only keep the group.
        _ => Inferred {
            model_type,
            context_window: None,
            max_output: None,
            can_vision: marked && model_type == ModelType::Chat,
            supports_tools: None,
            group,
        },
    }
}

/// Infers reasoning controls for supported model families and protocols; `base_url` picks
/// a vendor's own reasoning fields on its official OpenAI Chat endpoint.
#[must_use]
pub fn infer_reasoning(
    model_id: &str,
    protocol: Protocol,
    base_url: &str,
    max_output: Option<u32>,
) -> Option<ReasoningConfig> {
    let id = model_id.to_ascii_lowercase().replace('.', "-");
    let name = id.rsplit('/').next().unwrap_or_default();
    match protocol {
        Protocol::AnthropicMessages => infer_anthropic_reasoning(name, max_output),
        Protocol::OpenaiChat => {
            infer_vendor_reasoning(name, base_url).or_else(|| infer_openai_reasoning(name))
        }
        Protocol::OpenaiResponses => infer_openai_reasoning(name),
    }
}

/// Official vendor endpoints switch reasoning with their own fields (docs/28 RT4); a relay
/// serving the same model keeps the OpenAI dialect.
fn infer_vendor_reasoning(name: &str, base_url: &str) -> Option<ReasoningConfig> {
    use ReasoningEffort as E;
    let host = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split(['/', ':', '?'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let on = |domains: &[&str]| {
        domains.iter().any(|domain| {
            host == *domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
    };
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| name.starts_with(prefix));
    let switch = |efforts: &[ReasoningEffort]| ReasoningConfig {
        dialect: ReasoningDialect::ThinkingType,
        efforts: efforts.to_vec(),
        budget_tokens: None,
    };
    let toggle = [E::None, E::Auto];
    if on(&["deepseek.com"]) {
        if starts(&["deepseek-v4", "deepseek-v5"]) {
            return Some(switch(&[E::None, E::Low, E::High, E::Max]));
        }
        return starts(&["deepseek-chat", "deepseek-v3"]).then(|| switch(&toggle));
    }
    if on(&["bigmodel.cn", "z.ai"]) {
        if starts(&["glm-5-3"]) {
            return Some(switch(&[E::Low, E::High, E::Max]));
        }
        return starts(&["glm-5", "glm-4-5", "glm-4-6", "glm-4-7"]).then(|| switch(&toggle));
    }
    if on(&["volces.com"]) {
        return starts(&["doubao-seed"]).then(|| switch(&toggle));
    }
    if on(&["moonshot.cn", "moonshot.ai"]) {
        if starts(&["kimi-k3"]) {
            return Some(switch(&[E::None, E::Low, E::High, E::Max]));
        }
        return (starts(&[
            "kimi-k2-5",
            "kimi-k2-6",
            "kimi-k2-7",
            "kimi-k2-8",
            "kimi-k2-9",
        ]) && !name.contains("-code"))
        .then(|| switch(&toggle));
    }
    if on(&["aliyuncs.com"])
        && name.starts_with("qwen3")
        && !name.starts_with("qwen3-8-")
        && !["instruct", "coder", "thinking"]
            .iter()
            .any(|word| name.contains(word))
    {
        let cap = if starts(&["qwen3-max", "qwen3-5", "qwen3-6", "qwen3-7"]) {
            81_920
        } else {
            38_912
        };
        return Some(ReasoningConfig {
            dialect: ReasoningDialect::Qwen,
            efforts: vec![E::None, E::Auto, E::Low, E::Medium, E::High, E::Max],
            budget_tokens: Some(cap),
        });
    }
    None
}

fn infer_anthropic_reasoning(name: &str, max_output: Option<u32>) -> Option<ReasoningConfig> {
    let adaptive = [
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude-sonnet-4-7",
        "claude-sonnet-4-8",
        "claude-opus-5-5",
        "claude-opus-5",
        "claude-sonnet-5-5",
        "claude-sonnet-5",
        "claude-haiku-5-5",
        "claude-haiku-5",
    ];
    if let Some(prefix) = adaptive.iter().find(|prefix| version_match(name, prefix)) {
        let xhigh = prefix.ends_with("-5-5");
        let no_none = *prefix == "claude-opus-5-5";
        let mut efforts = Vec::with_capacity(if xhigh { 5 } else { 4 });
        if !no_none {
            efforts.push(ReasoningEffort::None);
        }
        efforts.extend([
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ]);
        if xhigh {
            efforts.push(ReasoningEffort::Xhigh);
        }
        efforts.push(ReasoningEffort::Max);
        return Some(ReasoningConfig {
            dialect: ReasoningDialect::AnthropicAdaptive,
            efforts,
            budget_tokens: None,
        });
    }

    let budget = [
        "claude-3-7-sonnet",
        "claude-sonnet-4-5",
        "claude-sonnet-4",
        "claude-opus-4-5",
        "claude-opus-4-1",
        "claude-opus-4",
        "claude-haiku-4-5",
    ]
    .iter()
    .any(|prefix| version_match(name, prefix));
    let budget_tokens = max_output.unwrap_or(8_193).saturating_sub(1).min(8_192);
    budget
        .then_some(ReasoningConfig {
            dialect: ReasoningDialect::AnthropicBudget,
            efforts: vec![
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            budget_tokens: (budget_tokens >= 1_024).then_some(budget_tokens),
        })
        .filter(|_| budget_tokens >= 1_024)
}

/// OpenAI Chat/Responses lines (official or relay) send `reasoning_effort`; a known
/// family keeps its efforts there, written in the OpenAI dialect.
fn infer_openai_reasoning(name: &str) -> Option<ReasoningConfig> {
    use ReasoningEffort as E;
    if name.starts_with("claude") {
        return infer_anthropic_reasoning(name, None)
            .map(|config| openai_reasoning(config.efforts));
    }
    if name.contains("non-reasoning") {
        return None;
    }
    let any = |prefixes: &[&str]| prefixes.iter().any(|prefix| version_match(name, prefix));
    let efforts: &[ReasoningEffort] =
        if (version_match(name, "o1") && !name.contains("-mini") && !name.contains("-preview"))
            || any(&["o3", "o4-mini", "gpt-oss"])
        {
            &[E::Low, E::Medium, E::High]
        } else if any(&["grok-4-3"]) {
            &[E::None, E::Low, E::Medium, E::High]
        } else if any(&["grok-4-6", "grok-4-7"]) {
            &[E::Low, E::Medium, E::High, E::Xhigh]
        } else if any(&["grok-3-mini"]) {
            &[E::Low, E::High]
        } else if any(&["gpt-5-1-codex-max"]) {
            &[E::Medium, E::High, E::Xhigh]
        } else if any(&["gpt-5-1-codex"]) {
            &[E::Medium, E::High]
        } else if any(&["gpt-5-codex"]) {
            &[E::Low, E::Medium, E::High]
        } else if any(&[
            "gpt-5-2-codex",
            "gpt-5-3-codex",
            "gpt-5-4-codex",
            "gpt-5-5-codex",
            "gpt-5-6-codex",
        ]) {
            &[E::Low, E::Medium, E::High, E::Xhigh]
        } else if name.contains("-chat") || name.contains("-pro") || name.contains("-codex") {
            return None;
        } else if any(&["gpt-5-1"]) {
            &[E::None, E::Low, E::Medium, E::High]
        } else if any(&["gpt-5-2", "gpt-5-3", "gpt-5-4", "gpt-5-5", "gpt-5-6"]) {
            &[E::None, E::Low, E::Medium, E::High, E::Xhigh]
        } else if any(&["gpt-5"]) {
            &[E::Minimal, E::Low, E::Medium, E::High]
        } else {
            return None;
        };
    Some(openai_reasoning(efforts.to_vec()))
}

fn version_match(name: &str, prefix: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(suffix) = rest.strip_prefix('-') else {
        return false;
    };
    let first = suffix.split('-').next().unwrap_or_default();
    !first.bytes().all(|byte| byte.is_ascii_digit()) || first.len() >= 4
}

fn openai_reasoning(efforts: Vec<ReasoningEffort>) -> ReasoningConfig {
    ReasoningConfig {
        dialect: ReasoningDialect::Openai,
        efforts,
        budget_tokens: None,
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        id: String,
        model_type: ModelType,
        context_window: Option<u32>,
        max_output: Option<u32>,
        can_vision: bool,
        supports_tools: Option<bool>,
        group: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ReasoningCase {
        id: String,
        protocol: Protocol,
        max_output: Option<u32>,
        /// Endpoint address; omitted means a relay (`https://relay.example/v1`).
        base_url: Option<String>,
        expect: Option<ReasoningConfig>,
    }

    #[test]
    fn every_reasoning_fixture_entry_matches() {
        let text = include_str!("../../../packages/ui/fixtures/reasoning-infer.json");
        let cases: Vec<ReasoningCase> = serde_json::from_str(text).unwrap();
        assert!(cases.len() >= 25);
        for case in cases {
            assert_eq!(
                infer_reasoning(
                    &case.id,
                    case.protocol,
                    case.base_url
                        .as_deref()
                        .unwrap_or("https://relay.example/v1"),
                    case.max_output
                ),
                case.expect,
                "{} {:?} {:?}",
                case.id,
                case.protocol,
                case.base_url
            );
        }
    }

    #[test]
    fn every_fixture_entry_matches() {
        let text = include_str!("../../../packages/ui/fixtures/model-classify.json");
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
