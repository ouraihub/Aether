//! Request encoding and stream decoding of the three wire protocols (docs/12 §3).

use aether_support::{Connection, Protocol, ReasoningEffort};
use reqwest::RequestBuilder;
use reqwest::header::ACCEPT;
use serde_json::{Map, Value, json};

use crate::run::Usage;
use crate::types::{Message, ToolCall, ToolSpec};

/// Endpoint path below the `/v1` base URL produced by providers.
pub(crate) const fn path(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::AnthropicMessages => "messages",
        Protocol::OpenaiChat => "chat/completions",
        Protocol::OpenaiResponses => "responses",
    }
}

/// One round's request: auth headers plus the streaming JSON body.
pub(crate) struct Request<'a> {
    pub(crate) connection: &'a Connection,
    pub(crate) model: &'a str,
    pub(crate) system: &'a str,
    pub(crate) history: &'a [Message],
    pub(crate) input: &'a str,
    /// Messages this run produced so far, after `input`.
    pub(crate) turn: &'a [Message],
    pub(crate) max_output: u32,
    pub(crate) tools: &'a [ToolSpec],
    pub(crate) native_web_search: bool,
    pub(crate) reasoning_effort: ReasoningEffort,
}

impl Request<'_> {
    pub(crate) fn encode(&self, builder: RequestBuilder) -> RequestBuilder {
        let key = self.connection.api_key.as_deref();
        let mut builder = builder.header(ACCEPT, "text/event-stream");
        if let Some(key) = key {
            builder = builder.bearer_auth(key.as_str());
        }
        let protocol = self.connection.protocol;
        let mut messages = Vec::new();
        encode_messages(protocol, self.history, &mut messages);
        push_user(&mut messages, self.input);
        encode_messages(protocol, self.turn, &mut messages);
        let has_system = !self.system.is_empty();
        let mut body = match protocol {
            Protocol::AnthropicMessages => {
                if let Some(key) = key {
                    builder = builder.header("x-api-key", key.as_str());
                }
                builder = builder.header("anthropic-version", "2023-06-01");
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
        if self.reasoning_effort != ReasoningEffort::Default {
            match protocol {
                Protocol::OpenaiChat => {
                    body["reasoning_effort"] = json!(self.reasoning_effort);
                }
                Protocol::OpenaiResponses => {
                    body["reasoning"] =
                        json!({ "effort": self.reasoning_effort, "summary": "auto" });
                }
                Protocol::AnthropicMessages => {
                    if self.reasoning_effort == ReasoningEffort::None {
                        body["thinking"] = json!({ "type": "disabled" });
                    } else if let Some(cap) = self
                        .connection
                        .reasoning
                        .as_ref()
                        .and_then(|config| config.budget_tokens)
                    {
                        let tokens = match self.reasoning_effort {
                            ReasoningEffort::Minimal => cap / 20,
                            ReasoningEffort::Low => cap / 10,
                            ReasoningEffort::Medium => cap / 2,
                            ReasoningEffort::High => (cap / 5) * 4 + (cap % 5) * 4 / 5,
                            ReasoningEffort::Xhigh => (cap / 10) * 9 + (cap % 10) * 9 / 10,
                            ReasoningEffort::Max => cap,
                            ReasoningEffort::Default | ReasoningEffort::None => 0,
                        };
                        let tokens = tokens.max(1024).min(self.max_output - 1);
                        body["thinking"] = json!({ "type": "enabled", "budget_tokens": tokens });
                    } else {
                        body["thinking"] = json!({ "type": "adaptive" });
                        body["output_config"] = json!({ "effort": self.reasoning_effort });
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
        builder.json(&body)
    }
}

fn encode_messages(protocol: Protocol, messages: &[Message], out: &mut Vec<Value>) {
    if protocol == Protocol::AnthropicMessages {
        anthropic_messages(messages, out);
    } else {
        for message in messages {
            openai_message(protocol, message, out);
        }
    }
}

/// Anthropic: consecutive tool results become one user message of `tool_result` blocks,
/// ordered like the `tool_use` blocks of the assistant message they answer.
fn anthropic_messages(history: &[Message], out: &mut Vec<Value>) {
    let is_tool = |m: &Message| matches!(m, Message::Tool { .. });
    let mut calls: &[ToolCall] = &[];
    for group in history.chunk_by(|a, b| is_tool(a) && is_tool(b)) {
        match group {
            [Message::User(text), ..] => push_user(out, text),
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
                content.extend(tool_calls.iter().map(
                    |c| json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.args }),
                ));
                out.push(json!({ "role": "assistant", "content": if content.is_empty() { json!(text) } else { json!(content) } }));
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
                            json!({ "type": "tool_result", "tool_use_id": call_id, "content": text }),
                        )),
                        Message::User(_) | Message::Assistant { .. } => None,
                    })
                    .collect();
                blocks.sort_by_key(|(position, _)| *position);
                let content: Vec<Value> = blocks.into_iter().map(|(_, block)| block).collect();
                out.push(json!({ "role": "user", "content": content }));
            }
        }
    }
}

/// Appends user text; after Anthropic tool results it joins that user message as a text block.
fn push_user(out: &mut Vec<Value>, text: &str) {
    if let Some(Value::Array(blocks)) = out
        .last_mut()
        .filter(|last| last["role"] == "user")
        .map(|last| &mut last["content"])
    {
        blocks.push(json!({ "type": "text", "text": text }));
    } else {
        out.push(json!({ "role": "user", "content": text }));
    }
}

fn openai_message(protocol: Protocol, message: &Message, out: &mut Vec<Value>) {
    let responses = protocol == Protocol::OpenaiResponses;
    match message {
        Message::User(text) => out.push(json!({ "role": "user", "content": text })),
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
            out.extend(tool_calls.iter().map(|c| json!({ "type": "function_call", "call_id": c.id, "name": c.name, "arguments": c.args.to_string() })));
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
                message["tool_calls"] = tool_calls.iter().map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.args.to_string() } })).collect();
            }
            out.push(message);
        }
        Message::Tool { call_id, text } if responses => {
            out.push(json!({ "type": "function_call_output", "call_id": call_id, "output": text }))
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

/// What one SSE event's data meant.
pub(crate) enum Decoded {
    Text(String),
    /// In-stream upstream error with its raw (unsanitized) message.
    Error(String),
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
pub(crate) struct Round {
    /// Calls in stream order; with `truncated`, none may run (unparsable args are `null`).
    pub(crate) tool_calls: Vec<ToolCall>,
    /// The output limit cut the reply, or some call's arguments are not a JSON object.
    pub(crate) truncated: bool,
}

/// Per-stream state; `done` is set by the protocol's terminal event.
pub(crate) struct Decoder {
    protocol: Protocol,
    pub(crate) done: bool,
    calls: Vec<Partial>,
    truncated: bool,
}

impl Decoder {
    pub(crate) const fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            done: false,
            calls: Vec::new(),
            truncated: false,
        }
    }

    /// Assembles the calls; each one's arguments must parse as a JSON object (docs/12 §3).
    pub(crate) fn finish(self) -> Round {
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

    pub(crate) fn event(&mut self, data: &str, usage: &mut Usage) -> Decoded {
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
                "message_start" => {
                    usage.input_tokens = tokens(&v["message"]["usage"]["input_tokens"]);
                }
                "content_block_delta" if v["delta"]["type"] == "text_delta" => {
                    return text(&v["delta"]["text"]);
                }
                "content_block_delta" if v["delta"]["type"] == "citations_delta" => {
                    return citation(&v["delta"]["citation"]);
                }
                "message_delta" => {
                    usage.output_tokens =
                        tokens(&v["usage"]["output_tokens"]).or(usage.output_tokens);
                    if v["delta"]["stop_reason"] == "max_tokens" {
                        self.truncated = true;
                    }
                }
                "message_stop" => self.done = true,
                "error" => return error(&v["error"]),
                _ => {}
            },
            Protocol::OpenaiChat => {
                if !v["error"].is_null() {
                    return error(&v["error"]);
                }
                if v["usage"].is_object() {
                    usage.input_tokens = tokens(&v["usage"]["prompt_tokens"]);
                    usage.output_tokens = tokens(&v["usage"]["completion_tokens"]);
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
                    usage.input_tokens = tokens(&v["response"]["usage"]["input_tokens"]);
                    usage.output_tokens = tokens(&v["response"]["usage"]["output_tokens"]);
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
    let Ok(parsed) = reqwest::Url::parse(url) else {
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
