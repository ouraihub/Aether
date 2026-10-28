//! One run over the protocol layer: streaming, timeouts, cancellation, single `end` (docs/12 §3, §7).

use std::collections::HashMap;
use std::mem;
use std::time::Duration;

use crate::native::ModelGateway;
use aether_support::net::AddressPolicy;
use aether_support::{KitError, KitErrorCode};
use tokio_util::sync::CancellationToken;

use crate::protocol::{Decoded, Request, Round};
use crate::tools::{self, Approver, AuthorizeContext, Settled, TRUNCATED_NOTE, ToolExecutor};
use crate::types::{
    Approval, EndStatus, Message, RunEvent, RunInput, RunSummary, ToolCall, ToolSource,
    ToolSourceKind, ToolSpec, Usage,
};
use crate::validate;

const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(120);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs model requests with the aether HTTP client.
pub struct RunEngine {
    gateway: ModelGateway,
    pub(crate) first_byte: Duration,
    pub(crate) idle: Duration,
}

impl RunEngine {
    /// A runner whose requests obey `policy` (no redirects, no proxy, vetted addresses).
    ///
    /// # Errors
    /// `internal` if the HTTP client cannot be built.
    pub fn new(policy: AddressPolicy) -> Result<Self, KitError> {
        Ok(Self {
            gateway: ModelGateway::new(policy)?,
            first_byte: FIRST_BYTE_TIMEOUT,
            idle: IDLE_TIMEOUT,
        })
    }

    /// Validates `input` and assembles its system prompt ([`RunEngine::validate`]), then runs
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
        let cost = aether_support::cost(input.connection.pricing.as_ref(), &turn.rounds);
        let (status, error, exhausted) = match result {
            Ok(exhausted) => {
                emitter.emit(RunEvent::Usage {
                    input_tokens: turn.usage.input_tokens,
                    output_tokens: turn.usage.output_tokens,
                    cache_read_tokens: turn.usage.cache_read_tokens,
                    cache_write_tokens: turn.usage.cache_write_tokens,
                    reasoning_tokens: turn.usage.reasoning_tokens,
                    cost,
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
            cost,
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
        let mut settled: HashMap<String, (String, serde_json::Value, String)> = HashMap::new();
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
                let text = if let Some((name, args, text)) = settled.get(&call.id) {
                    if name != &call.name || args != &call.args {
                        return Err(KitError::new(
                            KitErrorCode::Conflict,
                            "tool call id reused with different arguments",
                        )
                        .with_reason("tool_call_conflict"));
                    }
                    text.clone()
                } else {
                    let text = Self::call(run, catalogue, call, &boundary, emitter).await?;
                    settled.insert(
                        call.id.clone(),
                        (call.name.clone(), call.args.clone(), text.clone()),
                    );
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
        let ctx = AuthorizeContext {
            mode: run.input.permission_mode,
            purpose: &run.input.purpose,
            run_id: &run.input.run_id,
            effect: spec.effect,
            source: &spec.source,
            proposed: tools::proposed(run.input.permission_mode, spec),
        };
        // A catalogue `Deny` is final: no mode and no host answer turns it into a run.
        let decision = match spec.approval {
            Approval::Deny => Approval::Deny,
            Approval::Auto | Approval::Ask => run.tools.authorize(call, &ctx),
        };
        let allowed = match decision {
            Approval::Auto => true,
            Approval::Deny => false,
            Approval::Ask => {
                let wait = run.approver.wait(&run.input.run_id, call, run.cancel);
                emitter.emit(RunEvent::ApprovalRequired {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    args_preview: tools::args_preview(call),
                });
                // Also stops waiting on its own, should an approver ignore `cancel`.
                tokio::select! {
                    () = run.cancel.cancelled() => return Err(KitError::cancelled()),
                    allow = wait => allow,
                }
            }
        };
        let settled = if allowed {
            if run.cancel.is_cancelled() {
                return Err(KitError::cancelled());
            }
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
        // A request refused before streaming bills nothing; a streamed round without usage
        // makes the cost unknown.
        if result.is_ok() || usage != Usage::default() {
            turn.rounds.push(usage.into());
        }
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
        let known: Vec<&str> = connection.api_keys.iter().map(|key| key.as_str()).collect();
        self.gateway
            .stream(
                &connection.base_url,
                request.encode(),
                &known,
                (self.first_byte, self.idle),
                usage,
                |decoded| apply(decoded, emitter, text, &known),
            )
            .await
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
    /// Usage of every billed upstream request, for per-round pricing tiers.
    rounds: Vec<aether_support::TokenUsage>,
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
    decoded: Decoded,
    emitter: &mut Emitter<F>,
    text: &mut String,
    known: &[&str],
) -> Result<(), KitError> {
    match decoded {
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
