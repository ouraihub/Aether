use aether_support::Protocol;
use serde_json::{Map, Value};

use crate::types::{ToolCall, Usage};


/// What one SSE event's data meant.
pub enum Decoded {
    /// Assistant text delta.
    Text(String),
    /// In-stream upstream error with its raw (unsanitized) message.
    Error(String),
    /// Event does not produce display text.
    Skip,
}

/// A tool call whose arguments are still streaming in.
struct Partial {
    /// Per-protocol fragment key: anthropic block index, openai-chat `index`, responses `item_id`.
    key: Value,
    id: String,
    name: String,
    args: String,
}

/// What a finished stream asked for.
#[derive(Debug, PartialEq)]
pub struct Round {
    /// Calls in stream order; with `truncated`, none may run (unparsable args are `null`).
    pub tool_calls: Vec<ToolCall>,
    /// The output limit cut the reply, or some call's arguments are not a JSON object.
    pub truncated: bool,
}

/// Per-stream state; `done` is set by the protocol's terminal event.
pub struct Decoder {
    protocol: Protocol,
    /// Whether a protocol terminal event was received.
    pub done: bool,
    calls: Vec<Partial>,
    truncated: bool,
    /// Anthropic `input_tokens` without cache; `usage.input_tokens` adds cache back.
    base_input: Option<u32>,
}

impl Decoder {
    /// Starts one fresh protocol stream.
    pub const fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            done: false,
            calls: Vec::new(),
            truncated: false,
            base_input: None,
        }
    }

    /// Assembles the calls; each one's arguments must parse as a JSON object (docs/12 §3).
    pub fn finish(self) -> Round {
        let mut truncated = self.truncated;
        let tool_calls = self
            .calls
            .into_iter()
            .map(|call| {
                // No-argument tools may stream no fragment at all.
                let args = if call.args.is_empty() {
                    "{}"
                } else {
                    &call.args
                };
                let args = serde_json::from_str::<Map<String, Value>>(args).map_or_else(
                    |_| {
                        truncated = true;
                        Value::Null
                    },
                    Value::Object,
                );
                ToolCall {
                    id: call.id,
                    name: call.name,
                    args,
                }
            })
            .collect();
        Round {
            tool_calls,
            truncated,
        }
    }

    fn call(&mut self, key: &Value) -> Option<&mut Partial> {
        self.calls.iter_mut().find(|call| call.key == *key)
    }

    fn start(&mut self, key: &Value, id: &Value, name: &Value) {
        self.calls.push(Partial {
            key: key.clone(),
            id: id.as_str().unwrap_or_default().to_owned(),
            name: name.as_str().unwrap_or_default().to_owned(),
            args: String::new(),
        });
    }

    fn append(&mut self, key: &Value, fragment: &Value) {
        if let (Some(call), Some(fragment)) = (self.call(key), fragment.as_str()) {
            call.args.push_str(fragment);
        }
    }

    /// openai-chat: fragments of several calls may interleave; `id`/`name` come with the first.
    fn chat_tool_calls(&mut self, deltas: &Value) {
        for delta in deltas.as_array().into_iter().flatten() {
            let key = &delta["index"];
            let function = &delta["function"];
            if self.call(key).is_none() {
                self.start(key, &delta["id"], &function["name"]);
            }
            self.append(key, &function["arguments"]);
        }
    }

    /// Decodes one SSE data field and updates known usage.
    pub fn event(&mut self, data: &str, usage: &mut Usage) -> Decoded {
        if data == "[DONE]" {
            self.done = true;
            return Decoded::Skip;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Decoded::Skip;
        };
        let kind = v["type"].as_str().unwrap_or_default();
        match self.protocol {
            Protocol::AnthropicMessages => match kind {
                "content_block_start" if v["content_block"]["type"] == "tool_use" => {
                    let block = &v["content_block"];
                    self.start(&v["index"], &block["id"], &block["name"]);
                }
                "content_block_start"
                    if v["content_block"]["type"] == "web_search_tool_result"
                        && v["content_block"]["content"]["type"]
                            == "web_search_tool_result_error" =>
                {
                    return Decoded::Error("native web search failed".to_owned());
                }
                "content_block_delta" if v["delta"]["type"] == "input_json_delta" => {
                    self.append(&v["index"], &v["delta"]["partial_json"]);
                }
                "message_start" | "message_delta" => {
                    let raw = if kind == "message_start" {
                        &v["message"]["usage"]
                    } else {
                        &v["usage"]
                    };
                    self.base_input = tokens(&raw["input_tokens"]).or(self.base_input);
                    usage.output_tokens = tokens(&raw["output_tokens"]).or(usage.output_tokens);
                    usage.cache_read_tokens =
                        tokens(&raw["cache_read_input_tokens"]).or(usage.cache_read_tokens);
                    usage.cache_write_tokens =
                        tokens(&raw["cache_creation_input_tokens"]).or(usage.cache_write_tokens);
                    usage.reasoning_tokens =
                        tokens(&raw["reasoning_tokens"]).or(usage.reasoning_tokens);
                    // `inputTokens` always includes cache (docs/41 §4).
                    usage.input_tokens = self.base_input.map(|base| {
                        base.saturating_add(usage.cache_read_tokens.unwrap_or(0))
                            .saturating_add(usage.cache_write_tokens.unwrap_or(0))
                    });
                    if v["delta"]["stop_reason"] == "max_tokens" {
                        self.truncated = true;
                    }
                }
                "content_block_delta" if v["delta"]["type"] == "text_delta" => {
                    return text(&v["delta"]["text"]);
                }
                "content_block_delta" if v["delta"]["type"] == "citations_delta" => {
                    return citation(&v["delta"]["citation"]);
                }
                "message_stop" => self.done = true,
                "error" => return error(&v["error"]),
                _ => {}
            },
            Protocol::OpenaiChat => {
                if !v["error"].is_null() {
                    return error(&v["error"]);
                }
                let raw = &v["usage"];
                if raw.is_object() {
                    usage.input_tokens = tokens(&raw["prompt_tokens"]).or(usage.input_tokens);
                    usage.output_tokens = tokens(&raw["completion_tokens"]).or(usage.output_tokens);
                    usage.cache_read_tokens =
                        tokens(&raw["prompt_tokens_details"]["cached_tokens"])
                            .or(usage.cache_read_tokens);
                    usage.cache_write_tokens =
                        tokens(&raw["prompt_tokens_details"]["cache_write_tokens"])
                            .or(usage.cache_write_tokens);
                    usage.reasoning_tokens =
                        tokens(&raw["completion_tokens_details"]["reasoning_tokens"])
                            .or(usage.reasoning_tokens);
                }
                let choice = &v["choices"][0];
                self.chat_tool_calls(&choice["delta"]["tool_calls"]);
                if !choice["finish_reason"].is_null() {
                    self.done = true;
                    self.truncated |= choice["finish_reason"] == "length";
                }
                return text(&choice["delta"]["content"]);
            }
            Protocol::OpenaiResponses => match kind {
                "response.output_text.delta" => return text(&v["delta"]),
                "response.output_item.added" if v["item"]["type"] == "function_call" => {
                    let item = &v["item"];
                    // `call_id` (not the item id) is what `function_call_output` must echo.
                    self.start(&item["id"], &item["call_id"], &item["name"]);
                }
                "response.function_call_arguments.delta" => self.append(&v["item_id"], &v["delta"]),
                "response.output_text.annotation.added" => return citation(&v["annotation"]),
                "response.completed" | "response.incomplete" => {
                    let raw = &v["response"]["usage"];
                    usage.input_tokens = tokens(&raw["input_tokens"]);
                    usage.output_tokens = tokens(&raw["output_tokens"]);
                    usage.cache_read_tokens = tokens(&raw["input_tokens_details"]["cached_tokens"]);
                    usage.cache_write_tokens =
                        tokens(&raw["input_tokens_details"]["cache_write_tokens"]);
                    usage.reasoning_tokens =
                        tokens(&raw["output_tokens_details"]["reasoning_tokens"]);
                    self.done = true;
                    self.truncated |=
                        v["response"]["incomplete_details"]["reason"] == "max_output_tokens";
                }
                "response.failed" => return error(&v["response"]["error"]),
                "error" => return error(&v),
                _ => {}
            },
        }
        Decoded::Skip
    }
}

fn text(value: &Value) -> Decoded {
    match value.as_str() {
        Some(text) if !text.is_empty() => Decoded::Text(text.to_owned()),
        _ => Decoded::Skip,
    }
}

fn error(value: &Value) -> Decoded {
    Decoded::Error(
        value["message"]
            .as_str()
            .unwrap_or("upstream error")
            .to_owned(),
    )
}

fn citation(value: &Value) -> Decoded {
    let Some(url) = value["url"].as_str() else {
        return Decoded::Skip;
    };
    let Ok(parsed) = url::Url::parse(url) else {
        return Decoded::Skip;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return Decoded::Skip;
    }
    Decoded::Text(format!(
        " [source](<{}>)",
        parsed.as_str().replace('<', "%3C").replace('>', "%3E")
    ))
}

fn tokens(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}

#[cfg(test)]
mod summary_tests {
    use super::official_openai;

    #[test]
    fn only_openai_itself_gets_reasoning_summaries() {
        assert!(official_openai("https://api.openai.com/v1"));
        assert!(official_openai("https://api.openai.com"));
        assert!(!official_openai("https://api.openai.com.evil.example/v1"));
        assert!(!official_openai("http://api.openai.com/v1"));
        assert!(!official_openai("https://openrouter.ai/api/v1"));
    }
}