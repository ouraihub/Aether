//! One run over the protocol layer: streaming, timeouts, cancellation, single `end` (docs/12 §3, §7).

use std::collections::HashMap;
use std::mem;
use std::time::Duration;

use aether_support::net::{self, AddressPolicy};
use aether_support::sse::SseParser;
use aether_support::{KitError, KitErrorCode};
use reqwest::Response;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::protocol::{Decoded, Decoder, Request, Round, path};
use crate::tools::{self, Approver, Settled, TRUNCATED_NOTE, ToolExecutor};
use crate::types::{
    Approval, EndStatus, Message, RunEvent, RunInput, ToolCall, ToolSource, ToolSourceKind,
    ToolSpec,
};
use crate::validate;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(120);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest upstream error body read for the excerpt.
const ERROR_BODY_BYTES: usize = 8 * 1024;

/// Token usage reported by the upstream; `None` when not reported (never estimated).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: Option<u32>,
    /// Output tokens.
    pub output_tokens: Option<u32>,
    /// Cached input tokens.
    pub cache_read_tokens: Option<u32>,
    /// Cached write tokens.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning tokens.
    pub reasoning_tokens: Option<u32>,
}

impl Usage {
    /// Sums rounds; a side stays `None` only if no round reported it.
    fn add(&mut self, round: Self) {
        let sum = |a: Option<u32>, b: Option<u32>| match (a, b) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (a, b) => a.or(b),
        };
        self.input_tokens = sum(self.input_tokens, round.input_tokens);
        self.output_tokens = sum(self.output_tokens, round.output_tokens);
        self.cache_read_tokens = sum(self.cache_read_tokens, round.cache_read_tokens);
        self.cache_write_tokens = sum(self.cache_write_tokens, round.cache_write_tokens);
        self.reasoning_tokens = sum(self.reasoning_tokens, round.reasoning_tokens);
    }
}

/// Outcome and known usage of a completed or interrupted run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    /// Terminal status.
    pub status: EndStatus,
    /// Produced messages, including partial output.
    pub messages: Vec<Message>,
    /// Known provider-reported token counts.
    pub usage: Usage,
    /// Last request context estimate or report.
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Tool loop stopped at its configured round limit.
    pub exhausted: bool,
}

/// Runs model requests with the aether HTTP client.
pub struct Runner {
    http: reqwest::Client,
    policy: AddressPolicy,
    pub(crate) first_byte: Duration,
    pub(crate) idle: Duration,
}

impl Runner {
    /// A runner whose requests obey `policy` (no redirects, no proxy, vetted addresses).
    ///
    /// # Errors
    /// `internal` if the HTTP client cannot be built.
    pub fn new(policy: AddressPolicy) -> Result<Self, KitError> {
        Ok(Self {
            http: net::client(policy.clone(), CONNECT_TIMEOUT)?,
            policy,
            first_byte: FIRST_BYTE_TIMEOUT,
            idle: IDLE_TIMEOUT,
        })
    }

    /// Validates `input` and assembles its system prompt ([`Runner::validate`]), then runs
    /// the tool loop (docs/12 §5). Always emits exactly one `end` as the last event, also
    /// when validation fails or the future is dropped; the return value matches it.
    pub async fn run(
        &self,
        mut input: RunInput,
        tools: &dyn ToolExecutor,
        approver: &dyn Approver,
        cancel: &CancellationToken,
        on: impl FnMut(RunEvent) + Send,
    ) -> RunSummary {
        let mut emitter = Emitter { on, ended: false };
        let mut turn = Turn::default();
        let result = match validate::prepare(&mut input) {
            Ok(system) => {
                let run = Ctx {
                    input: &input,
                    system: &system,
                    tools,
                    approver,
                    cancel,
                };
                self.rounds(&run, &mut emitter, &mut turn).await
            }
            Err(error) => Err(error),
        };
        turn.flush();
        let (status, error, exhausted) = match result {
            Ok(exhausted) => {
                emitter.emit(RunEvent::Usage {
                    input_tokens: turn.usage.input_tokens,
                    output_tokens: turn.usage.output_tokens,
                    cache_read_tokens: turn.usage.cache_read_tokens,
                    cache_write_tokens: turn.usage.cache_write_tokens,
                    reasoning_tokens: turn.usage.reasoning_tokens,
                });
                (EndStatus::Completed, None, exhausted)
            }
            Err(error) if error.code() == KitErrorCode::Cancelled => {
                (EndStatus::Cancelled, None, false)
            }
            Err(error) => {
                tracing::warn!(run_id = %input.run_id, code = ?error.code(), "chat.run.failed");
                (EndStatus::Failed, Some(error), false)
            }
        };
        emitter.ended = true;
        (emitter.on)(RunEvent::End {
            status,
            error,
            exhausted: exhausted.then_some(true),
        });
        RunSummary {
            status,
            messages: turn.messages,
            usage: turn.usage,
            context_usage: None,
            exhausted,
        }
    }

    /// The loop of docs/12 §5; `Ok(true)` when the round limit stopped it.
    async fn rounds<F: FnMut(RunEvent)>(
        &self,
        run: &Ctx<'_>,
        emitter: &mut Emitter<F>,
        turn: &mut Turn,
    ) -> Result<bool, KitError> {
        // Fed back again for a repeated call id instead of executing twice (docs/12 §5).
        let mut settled: HashMap<String, String> = HashMap::new();
        let boundary = validate::boundary_id()?;
        let mut catalogue = run.input.tools.as_slice();
        for round in 1..=run.input.budget.max_rounds {
            // No new request after a cancel (docs/12 §5 step 7).
            if run.cancel.is_cancelled() {
                return Err(KitError::cancelled());
            }
            emitter.emit(RunEvent::RoundStart { round });
            let mut result = self.stream(run, catalogue, emitter, turn).await;
            if result.as_ref().is_err_and(rejects_tools)
                && !catalogue.is_empty()
                && run.input.connection.supports_tools.is_none()
                && turn.text.is_empty()
            {
                tracing::warn!(run_id = %run.input.run_id, "chat.run.tools_rejected_retry");
                catalogue = &[];
                result = self.stream(run, catalogue, emitter, turn).await;
            }
            let Round {
                tool_calls,
                truncated,
            } = result?;
            if tool_calls.is_empty() {
                turn.flush();
                return Ok(false);
            }
            if truncated {
                // Arguments may be cut off: execute none of them (docs/12 §8 trap 1).
                turn.flush();
                turn.messages.push(Message::User(TRUNCATED_NOTE.to_owned()));
                continue;
            }
            turn.messages.push(Message::Assistant {
                text: mem::take(&mut turn.text),
                tool_calls: tool_calls.clone(),
                reasoning: Vec::new(),
            });
            for call in &tool_calls {
                if run.cancel.is_cancelled() {
                    return Err(KitError::cancelled());
                }
                let text = if let Some(text) = settled.get(&call.id) {
                    text.clone()
                } else {
                    let text = Self::call(run, catalogue, call, &boundary, emitter).await?;
                    settled.insert(call.id.clone(), text.clone());
                    text
                };
                turn.messages.push(Message::Tool {
                    call_id: call.id.clone(),
                    text,
                });
            }
            if run.cancel.is_cancelled() {
                return Err(KitError::cancelled());
            }
        }
        Ok(true)
    }

    /// One authorized, possibly approved call; returns the text fed back to the model.
    async fn call<F: FnMut(RunEvent)>(
        run: &Ctx<'_>,
        catalogue: &[ToolSpec],
        call: &ToolCall,
        boundary: &str,
        emitter: &mut Emitter<F>,
    ) -> Result<String, KitError> {
        let Some(spec) = catalogue.iter().find(|spec| spec.name == call.name) else {
            return Ok(tools::unknown_tool(call));
        };
        let (source, server) = match &spec.source {
            ToolSource::Host => (ToolSourceKind::Host, None),
            ToolSource::Mcp { server } => (ToolSourceKind::Mcp, Some(server.clone())),
        };
        emitter.emit(RunEvent::ToolCall {
            call_id: call.id.clone(),
            name: call.name.clone(),
            source,
            server,
        });
        let allowed = match run.tools.authorize(call, spec.approval) {
            Approval::Auto => true,
            Approval::Deny => false,
            Approval::Ask => {
                emitter.emit(RunEvent::ApprovalRequired {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    args_preview: tools::args_preview(call),
                });
                // Also stops waiting on its own, should an approver ignore `cancel`.
                tokio::select! {
                    () = run.cancel.cancelled() => return Err(KitError::cancelled()),
                    allow = run.approver.wait(&run.input.run_id, call, run.cancel) => allow,
                }
            }
        };
        let settled = if allowed {
            // An in-flight tool is awaited even after a cancel; it reports `unknown` itself.
            let execution = run.tools.execute(call, run.cancel).await;
            Settled::from_execution(
                execution,
                spec,
                run.input.budget.tool_output_chars,
                boundary,
            )
        } else {
            Settled::denied()
        };
        emitter.emit(RunEvent::ToolResult {
            call_id: call.id.clone(),
            outcome: settled.outcome,
            summary: settled.summary,
            payload: settled.payload,
        });
        Ok(settled.text)
    }

    /// One model request, interrupted at once by `cancel`.
    async fn stream<F: FnMut(RunEvent)>(
        &self,
        run: &Ctx<'_>,
        tools: &[ToolSpec],
        emitter: &mut Emitter<F>,
        turn: &mut Turn,
    ) -> Result<Round, KitError> {
        let mut usage = Usage::default();
        let result = tokio::select! {
            () = run.cancel.cancelled() => Err(KitError::cancelled()),
            result = self.round(run, tools, &turn.messages, emitter, &mut turn.text, &mut usage) => result,
        };
        turn.usage.add(usage);
        result
    }

    async fn round<F: FnMut(RunEvent)>(
        &self,
        run: &Ctx<'_>,
        tools: &[ToolSpec],
        messages: &[Message],
        emitter: &mut Emitter<F>,
        text: &mut String,
        usage: &mut Usage,
    ) -> Result<Round, KitError> {
        let input = run.input;
        let connection = &input.connection;
        let model = connection
            .model_id
            .as_deref()
            .ok_or_else(|| KitError::needs_config("missing_model"))?;
        let base = connection.base_url.trim_end_matches('/');
        let url = net::check_url(
            &format!("{base}/{}", path(connection.protocol)),
            &self.policy,
        )?;
        let request = Request {
            connection,
            model,
            system: run.system,
            history: &input.history,
            input: &input.input,
            images: &input.images,
            turn: messages,
            max_output: input.budget.max_output,
            tools,
            native_web_search: input.native_web_search,
            reasoning_effort: input.reasoning_effort,
        };
        let known: Vec<&str> = connection.api_key.iter().map(|key| key.as_str()).collect();
        let first_byte = async {
            let mut response = request
                .encode(self.http.post(url))
                .send()
                .await
                .map_err(transport)?;
            if !response.status().is_success() {
                return Err(upstream(response, &known).await);
            }
            let chunk = response.chunk().await.map_err(transport)?;
            Ok((response, chunk))
        };
        let (mut response, mut chunk) = timeout(self.first_byte, first_byte)
            .await
            .map_err(|_| timed_out("first_byte"))??;
        let mut parser = SseParser::new();
        let mut decoder = Decoder::new(connection.protocol);
        while let Some(bytes) = chunk {
            for event in parser.feed(&bytes) {
                apply(&mut decoder, &event.data, emitter, text, usage, &known)?;
            }
            chunk = timeout(self.idle, response.chunk())
                .await
                .map_err(|_| timed_out("idle"))?
                .map_err(transport)?;
        }
        for event in parser.finish() {
            apply(&mut decoder, &event.data, emitter, text, usage, &known)?;
        }
        if decoder.done {
            Ok(decoder.finish())
        } else {
            Err(KitError::new(KitErrorCode::Network, "stream ended early")
                .with_reason("incomplete_stream"))
        }
    }
}

/// What one run needs besides the emitter.
struct Ctx<'a> {
    input: &'a RunInput,
    system: &'a str,
    tools: &'a dyn ToolExecutor,
    approver: &'a dyn Approver,
    cancel: &'a CancellationToken,
}

/// This run's output so far; `text` is the round being streamed.
#[derive(Default)]
struct Turn {
    messages: Vec<Message>,
    text: String,
    usage: Usage,
}

impl Turn {
    /// Keeps streamed text (also partial text of a failed or cancelled round).
    fn flush(&mut self) {
        if !self.text.is_empty() {
            self.messages.push(Message::Assistant {
                text: mem::take(&mut self.text),
                tool_calls: Vec::new(),
                reasoning: Vec::new(),
            });
        }
    }
}

/// A 400 that names tools: the upstream does not take them (docs/12 §5).
fn rejects_tools(error: &KitError) -> bool {
    error.details().status() == Some(400) && error.message().to_ascii_lowercase().contains("tool")
}

fn apply<F: FnMut(RunEvent)>(
    decoder: &mut Decoder,
    data: &str,
    emitter: &mut Emitter<F>,
    text: &mut String,
    usage: &mut Usage,
    known: &[&str],
) -> Result<(), KitError> {
    match decoder.event(data, usage) {
        Decoded::Text(delta) => {
            text.push_str(&delta);
            emitter.emit(RunEvent::TextDelta { text: delta });
            Ok(())
        }
        // Arrives after a 200, so there is no HTTP status to classify by.
        Decoded::Error(message) => {
            Err(KitError::upstream(200, &message, known).with_reason("stream_error"))
        }
        Decoded::Skip => Ok(()),
    }
}

/// Emits `end{cancelled}` (or `failed` while panicking) if the run never sent its own.
struct Emitter<F: FnMut(RunEvent)> {
    on: F,
    ended: bool,
}

impl<F: FnMut(RunEvent)> Emitter<F> {
    fn emit(&mut self, event: RunEvent) {
        (self.on)(event);
    }
}

impl<F: FnMut(RunEvent)> Drop for Emitter<F> {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let (status, error) = if std::thread::panicking() {
            (
                EndStatus::Failed,
                Some(KitError::new(KitErrorCode::Internal, "run panicked")),
            )
        } else {
            (EndStatus::Cancelled, None)
        };
        (self.on)(RunEvent::End {
            status,
            error,
            exhausted: None,
        });
    }
}

fn timed_out(reason: &str) -> KitError {
    KitError::new(KitErrorCode::Timeout, format!("upstream {reason} timeout")).with_reason(reason)
}

fn transport(error: reqwest::Error) -> KitError {
    let error = error.without_url();
    if net::is_blocked(&error) {
        return KitError::bad_request("url", "blocked_address").with_cause(error);
    }
    let code = if error.is_timeout() {
        KitErrorCode::Timeout
    } else {
        KitErrorCode::Network
    };
    KitError::new(code, "upstream unreachable").with_cause(error)
}

async fn upstream(mut response: Response, known: &[&str]) -> KitError {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    // Best effort: a broken error body still yields the status.
    while body.len() < ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) | Err(_) => break,
        }
    }
    body.truncate(ERROR_BODY_BYTES);
    KitError::upstream(status, &String::from_utf8_lossy(&body), known)
}
