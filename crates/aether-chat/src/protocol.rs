//! Request encoding and stream decoding of the three wire protocols (docs/12 §3).
mod decoder;
pub use decoder::{Decoded, Decoder, Round};

use std::iter;

use aether_support::{Connection, Protocol, ReasoningDialect, ReasoningEffort};
use serde_json::{Map, Value, json};

use crate::types::{Message, ToolCall, ToolSpec, Usage};
use aether_images::ImagePart;

/// Endpoint path below the `/v1` base URL produced by providers.
pub const fn path(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::AnthropicMessages => "messages",
        Protocol::OpenaiChat => "chat/completions",
        Protocol::OpenaiResponses => "responses",
    }
}

/// Share of a reasoning budget an explicit effort asks for.
fn scaled(cap: u32, effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Minimal => cap / 20,
        ReasoningEffort::Low => cap / 10,
        ReasoningEffort::Medium => cap / 2,
        ReasoningEffort::High => (cap / 5) * 4 + (cap % 5) * 4 / 5,
        ReasoningEffort::Xhigh => (cap / 10) * 9 + (cap % 10) * 9 / 10,
        ReasoningEffort::Max => cap,
        ReasoningEffort::Default | ReasoningEffort::None | ReasoningEffort::Auto => 0,
    }
}

/// Whether `base_url` is OpenAI's own API, the only Responses server known to accept `summary`.
fn official_openai(base_url: &str) -> bool {
    base_url
        .strip_prefix("https://api.openai.com")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// One round's request: auth headers plus the streaming JSON body.
pub struct Request<'a> {
    /// Resolved model connection; credentials are not serialized into the description.
    pub connection: &'a Connection,
    /// Model identifier.
    pub model: &'a str,
    /// Assembled system prompt.
    pub system: &'a str,
    /// Prepared prior messages.
    pub history: &'a [Message],
    /// Current input text.
    pub input: &'a str,
    /// Images of `input`; the runner already rejected them for a model without vision.
    pub images: &'a [ImagePart],
    /// Messages this run produced so far, after `input`.
    pub turn: &'a [Message],
    /// Maximum output token budget.
    pub max_output: u32,
    /// Fixed tool catalogue.
    pub tools: &'a [ToolSpec],
    /// Whether to enable the protocol's native search.
    pub native_web_search: bool,
    /// Requested reasoning control.
    pub reasoning_effort: ReasoningEffort,
}

impl Request<'_> {
    /// Encodes one pure request description without constructing an HTTP request.
    pub fn encode(&self) -> Description<'_> {
        let protocol = self.connection.protocol;
        let mut messages = Vec::new();
        let vision = self.connection.can_vision;
        encode_messages(protocol, vision, self.history, &mut messages);
        push_user(&mut messages, protocol, self.input, self.images);
        encode_messages(protocol, vision, self.turn, &mut messages);
        let has_system = !self.system.is_empty();
        let mut body = match protocol {
            Protocol::AnthropicMessages => {
                let mut body = json!({
                    "model": self.model, "max_tokens": self.max_output,
                    "stream": true, "messages": messages,
                });
                if has_system {
                    body["system"] = json!(self.system);
                }
                body
            }
            Protocol::OpenaiChat => {
                if has_system {
                    messages.insert(0, json!({ "role": "system", "content": self.system }));
                }
                json!({
                    "model": self.model, "max_tokens": self.max_output, "stream": true,
                    "stream_options": { "include_usage": true }, "messages": messages,
                })
            }
            Protocol::OpenaiResponses => {
                let mut body = json!({
                    "model": self.model, "max_output_tokens": self.max_output,
                    "stream": true, "input": messages,
                });
                if has_system {
                    body["instructions"] = json!(self.system);
                }
                body
            }
        };
        let effort = self.reasoning_effort;
        let config = self.connection.reasoning.as_ref();
        if effort != ReasoningEffort::Default {
            match protocol {
                Protocol::OpenaiChat => match config.map(|config| config.dialect) {
                    Some(ReasoningDialect::ThinkingType) => {
                        let on = effort != ReasoningEffort::None;
                        body["thinking"] =
                            json!({ "type": if on { "enabled" } else { "disabled" } });
                        if on && effort != ReasoningEffort::Auto {
                            body["reasoning_effort"] = json!(effort);
                        }
                    }
                    Some(ReasoningDialect::Qwen) => {
                        body["enable_thinking"] = json!(effort != ReasoningEffort::None);
                        if let Some(cap) = config.and_then(|config| config.budget_tokens)
                            && !matches!(effort, ReasoningEffort::None | ReasoningEffort::Auto)
                        {
                            body["thinking_budget"] = json!(scaled(cap, effort).max(1));
                        }
                    }
                    _ => body["reasoning_effort"] = json!(effort),
                },
                Protocol::OpenaiResponses => {
                    // Third-party Responses servers may reject `summary` with 400.
                    body["reasoning"] = if official_openai(&self.connection.base_url) {
                        json!({ "effort": effort, "summary": "auto" })
                    } else {
                        json!({ "effort": effort })
                    };
                }
                Protocol::AnthropicMessages => {
                    if effort == ReasoningEffort::None {
                        body["thinking"] = json!({ "type": "disabled" });
                    } else if let Some(cap) = config.and_then(|config| config.budget_tokens) {
                        let tokens = scaled(cap, effort).max(1024).min(self.max_output - 1);
                        body["thinking"] = json!({ "type": "enabled", "budget_tokens": tokens });
                    } else {
                        body["thinking"] = json!({ "type": "adaptive" });
                        body["output_config"] = json!({ "effort": effort });
                    }
                }
            }
        }
        let mut tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| tool_json(protocol, tool))
            .collect();
        if self.native_web_search && protocol == Protocol::AnthropicMessages {
            tools.push(json!({"type":"web_search_20250305","name":"web_search","max_uses":5}));
        } else if self.native_web_search && protocol == Protocol::OpenaiResponses {
            tools.push(json!({"type":"web_search"}));
        }
        if !tools.is_empty() {
            body["tools"] = tools.into();
        }
        Description {
            body,
            headers: &self.connection.headers,
            protocol,
        }
    }
}

/// Pure request body and borrowed header plan; credentials are supplied per attempt.
pub struct Description<'a> {
    /// Encoded protocol JSON body.
    pub body: Value,
    /// Custom headers, applied before accept and per-attempt authentication.
    pub headers: &'a [(String, String)],
    /// Authentication plan: Bearer, plus Anthropic API key/version for Messages.
    pub protocol: Protocol,
}

/// `vision = false` strips history images: only their text is sent (docs/19 §3).
fn encode_messages(protocol: Protocol, vision: bool, messages: &[Message], out: &mut Vec<Value>) {
    if protocol == Protocol::AnthropicMessages {
        anthropic_messages(vision, messages, out);
    } else {
        for message in messages {
            openai_message(protocol, vision, message, out);
        }
    }
}

/// Anthropic: consecutive tool results become one user message of `tool_result` blocks,
/// ordered like the `tool_use` blocks of the assistant message they answer.
fn anthropic_messages(vision: bool, history: &[Message], out: &mut Vec<Value>) {
    let is_tool = |m: &Message| matches!(m, Message::Tool { .. });
    let mut calls: &[ToolCall] = &[];
    for group in history.chunk_by(|a, b| is_tool(a) && is_tool(b)) {
        match group {
            [Message::User(text), ..] => push_user(out, Protocol::AnthropicMessages, text, &[]),
            [Message::UserImages { text, images }, ..] => push_user(
                out,
                Protocol::AnthropicMessages,
                text,
                if vision { images } else { &[] },
            ),
            [
                Message::Assistant {
                    text,
                    tool_calls,
                    reasoning,
                },
                ..,
            ] => {
                calls = tool_calls;
                let mut content = Vec::new();
                for block in reasoning {
                    if block.protocol == Protocol::AnthropicMessages {
                        content.push(block.value.clone());
                    }
                }
                if !text.is_empty() {
                    content.push(json!({ "type": "text", "text": text }));
                }
                content.extend(tool_calls.iter().map(|c| {
                    json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": c.args,
                    })
                }));
                out.push(json!({
                    "role": "assistant",
                    "content": if content.is_empty() { json!(text) } else { json!(content) },
                }));
            }
            results => {
                let mut blocks: Vec<(usize, Value)> = results
                    .iter()
                    .filter_map(|m| match m {
                        Message::Tool { call_id, text } => Some((
                            calls
                                .iter()
                                .position(|c| c.id == *call_id)
                                .unwrap_or(usize::MAX),
                            json!({
                                "type": "tool_result",
                                "tool_use_id": call_id,
                                "content": text,
                            }),
                        )),
                        Message::User(_)
                        | Message::UserImages { .. }
                        | Message::Assistant { .. } => None,
                    })
                    .collect();
                blocks.sort_by_key(|(position, _)| *position);
                let content: Vec<Value> = blocks.into_iter().map(|(_, block)| block).collect();
                out.push(json!({ "role": "user", "content": content }));
            }
        }
    }
}

/// Appends user content (docs/19 §4); after Anthropic tool results it joins that user
/// message as blocks.
fn push_user(out: &mut Vec<Value>, protocol: Protocol, text: &str, images: &[ImagePart]) {
    if let Some(Value::Array(blocks)) = out
        .last_mut()
        .filter(|last| protocol == Protocol::AnthropicMessages && last["role"] == "user")
        .map(|last| &mut last["content"])
    {
        blocks.extend(user_blocks(protocol, text, images));
    } else {
        out.push(json!({ "role": "user", "content": user_content(protocol, text, images) }));
    }
}

/// Plain string without images, so text-only request bodies stay byte-identical.
fn user_content(protocol: Protocol, text: &str, images: &[ImagePart]) -> Value {
    if images.is_empty() {
        json!(text)
    } else {
        user_blocks(protocol, text, images).into()
    }
}

/// Anthropic puts images before the text (its guidance); OpenAI text first.
fn user_blocks(protocol: Protocol, text: &str, images: &[ImagePart]) -> Vec<Value> {
    let url = |image: &ImagePart| format!("data:{};base64,{}", image.mime, image.data);
    match protocol {
        Protocol::AnthropicMessages => images
            .iter()
            .map(|image| {
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": image.mime,
                        "data": image.data,
                    },
                })
            })
            .chain(iter::once(json!({ "type": "text", "text": text })))
            .collect(),
        Protocol::OpenaiChat => iter::once(json!({ "type": "text", "text": text }))
            .chain(images.iter().map(|image| {
                json!({
                    "type": "image_url",
                    "image_url": { "url": url(image) },
                })
            }))
            .collect(),
        Protocol::OpenaiResponses => iter::once(json!({
            "type": "input_text",
            "text": text,
        }))
        .chain(images.iter().map(|image| {
            json!({ "type": "input_image", "image_url": url(image) })
        }))
        .collect(),
    }
}

fn openai_message(protocol: Protocol, vision: bool, message: &Message, out: &mut Vec<Value>) {
    let responses = protocol == Protocol::OpenaiResponses;
    match message {
        Message::User(text) => out.push(json!({ "role": "user", "content": text })),
        Message::UserImages { text, images } => out.push(json!({
            "role": "user",
            "content": user_content(protocol, text, if vision { images } else { &[] }),
        })),
        Message::Assistant {
            text,
            tool_calls,
            reasoning,
        } if responses => {
            for block in reasoning {
                if block.protocol == Protocol::OpenaiResponses {
                    out.push(block.value.clone());
                }
            }
            if !text.is_empty() || tool_calls.is_empty() {
                out.push(json!({ "role": "assistant", "content": text }));
            }
            out.extend(tool_calls.iter().map(|c| json!({
                "type": "function_call",
                "call_id": c.id,
                "name": c.name,
                "arguments": c.args.to_string(),
            })));
        }
        Message::Assistant {
            text,
            tool_calls,
            reasoning,
        } => {
            let mut message = json!({ "role": "assistant", "content": text });
            if !reasoning.is_empty() {
                message["reasoning_content"] = reasoning
                    .iter()
                    .filter(|b| b.protocol == Protocol::OpenaiChat)
                    .filter_map(|b| b.value.as_str())
                    .collect::<String>()
                    .into();
            }
            if !tool_calls.is_empty() {
                message["tool_calls"] = tool_calls
                    .iter()
                    .map(|c| json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": c.args.to_string(),
                        },
                    }))
                    .collect();
            }
            out.push(message);
        }
        Message::Tool { call_id, text } if responses => {
            out.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": text,
            }))
        }
        Message::Tool { call_id, text } => {
            out.push(json!({ "role": "tool", "tool_call_id": call_id, "content": text }));
        }
    }
}

fn tool_json(protocol: Protocol, tool: &ToolSpec) -> Value {
    let (name, description, parameters) = (&tool.name, &tool.description, &tool.parameters);
    match protocol {
        Protocol::AnthropicMessages => {
            json!({ "name": name, "description": description, "input_schema": parameters })
        }
        Protocol::OpenaiChat => json!({
            "type": "function",
            "function": { "name": name, "description": description, "parameters": parameters },
        }),
        Protocol::OpenaiResponses => json!({
            "type": "function", "name": name, "description": description,
            "parameters": parameters,
        }),
    }
}
