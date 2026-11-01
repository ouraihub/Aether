//! One run over the protocol layer: streaming, timeouts, cancellation, single `end` (docs/12 §3, §7).
//!
//! The decisions live in [`Machine`]; this file performs its actions (model requests, host
//! authorization, approval waits, tool execution) and owns the run-level output: usage,
//! cost and the single `end`.

use std::time::Duration;

use crate::machine::{Action, Machine};
use crate::native::ModelGateway;
use aether_support::net::AddressPolicy;
use aether_support::{ContextUsage, KitError, KitErrorCode};
use tokio_util::sync::CancellationToken;

use crate::protocol::{Decoded, Request, Round};
use crate::tools::{Approver, ToolExecutor};
use crate::types::{EndStatus, Message, RunEvent, RunInput, RunSummary, ToolSpec, Usage};
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
        let mut meter = Meter {
            context: Some(ContextUsage::for_request(
                Default::default(),
                None,
                input.connection.context_window,
                input.connection.endpoint_id.clone(),
                input.connection.model_id.clone(),
            )),
            ..Meter::default()
        };
        let (result, messages) = match validate::prepare(&mut input) {
            Ok(system) => {
                let run = Ctx {
                    input: &input,
                    system: &system,
                    tools,
                    approver,
                    cancel,
                };
                self.drive(&run, &mut emitter, &mut meter).await
            }
            Err(error) => (Err(error), Vec::new()),
        };
        let cost = aether_support::cost(input.connection.pricing.as_ref(), &meter.rounds);
        let (status, error, exhausted) = match result {
            Ok(exhausted) => {
                emitter.emit(RunEvent::Usage {
                    input_tokens: meter.usage.input_tokens,
                    output_tokens: meter.usage.output_tokens,
                    cache_read_tokens: meter.usage.cache_read_tokens,
                    cache_write_tokens: meter.usage.cache_write_tokens,
                    reasoning_tokens: meter.usage.reasoning_tokens,
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
        if status != EndStatus::Completed {
            meter.context = Some(ContextUsage::for_request(
                Default::default(),
                None,
                input.connection.context_window,
                input.connection.endpoint_id.clone(),
                input.connection.model_id.clone(),
            ));
        }
        if meter.started
            && let Some(context) = &meter.context
        {
            emitter.emit(RunEvent::Context {
                usage: context.clone(),
            });
        }
        emitter.ended = true;
        (emitter.on)(RunEvent::End {
            status,
            error,
            exhausted: exhausted.then_some(true),
        });
        RunSummary {
            status,
            messages,
            usage: meter.usage,
            cost,
            context_usage: meter.context,
            exhausted,
        }
    }

    /// Performs the machine's actions until it is done (docs/12 §5); `Ok(true)` when the
    /// round limit stopped the loop. Returns the turn's messages either way.
    async fn drive<F: FnMut(RunEvent)>(
        &self,
        run: &Ctx<'_>,
        emitter: &mut Emitter<F>,
        meter: &mut Meter,
    ) -> (Result<bool, KitError>, Vec<Message>) {
        let boundary = match validate::boundary_id() {
            Ok(boundary) => boundary,
            Err(error) => return (Err(error), Vec::new()),
        };
        let mut machine = Machine::new(run.input, boundary, run.cancel);
        // The round being streamed; the machine takes it into the turn when the round ends.
        let mut text = String::new();
        let mut emit = |event| emitter.emit(event);
        let mut action = machine.start(&mut emit);
        let result = loop {
            action = match action {
                Action::Model => {
                    let result = self
                        .stream(
                            run,
                            machine.catalogue(),
                            machine.messages(),
                            &mut emit,
                            &mut text,
                            meter,
                        )
                        .await;
                    machine.model_done(result, &mut text, &mut emit)
                }
                Action::Authorize(pending) => {
                    let ctx = machine.authorize_context(&pending);
                    let decision = run.tools.authorize(&pending.call, &ctx);
                    machine.authorized(pending, decision, &mut emit)
                }
                Action::Approve(pending, event) => {
                    let wait = run
                        .approver
                        .wait(&run.input.run_id, &pending.call, run.cancel);
                    emit(event);
                    // Also stops waiting on its own, should an approver ignore `cancel`. Biased:
                    // an approver answering `false` because of the same cancel is a cancel.
                    let allow = tokio::select! {
                        biased;
                        () = run.cancel.cancelled() => None,
                        allow = wait => Some(allow),
                    };
                    machine.approved(pending, allow, &mut emit)
                }
                Action::Execute(pending) => {
                    let execution = run.tools.execute(&pending.call, run.cancel).await;
                    machine.executed(pending, execution, &mut emit)
                }
                Action::Done(result) => break result,
            };
        };
        // Partial text of a failed or cancelled round stays in the turn.
        machine.flush(&mut text);
        (result, machine.into_messages())
    }

    /// One model request, interrupted at once by `cancel`.
    async fn stream(
        &self,
        run: &Ctx<'_>,
        tools: &[ToolSpec],
        messages: &[Message],
        emit: &mut impl FnMut(RunEvent),
        text: &mut String,
        meter: &mut Meter,
    ) -> Result<Round, KitError> {
        meter.started = true;
        let mut usage = Usage::default();
        let mut estimated = None;
        let result = tokio::select! {
            biased;
            () = run.cancel.cancelled() => Err(KitError::cancelled()),
            result = self.round(run, tools, messages, emit, text, (&mut usage, &mut estimated)) => result,
        };
        meter.usage.add(usage);
        // A request refused before streaming bills nothing; a streamed round without usage
        // makes the cost unknown.
        if result.is_ok() || usage != Usage::default() {
            meter.rounds.push(usage.into());
            meter.context = Some(ContextUsage::for_request(
                usage.into(),
                estimated,
                run.input.connection.context_window,
                run.input.connection.endpoint_id.clone(),
                run.input.connection.model_id.clone(),
            ));
            if let Some(context) = &meter.context {
                emit(RunEvent::Context {
                    usage: context.clone(),
                });
            }
        }
        result
    }

    async fn round(
        &self,
        run: &Ctx<'_>,
        tools: &[ToolSpec],
        messages: &[Message],
        emit: &mut impl FnMut(RunEvent),
        text: &mut String,
        accounting: (&mut Usage, &mut Option<u32>),
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
        let encoded = request.encode();
        *accounting.1 = Some(estimate_request(&encoded.body));
        let known: Vec<&str> = connection.api_keys.iter().map(|key| key.as_str()).collect();
        self.gateway
            .stream(
                &connection.base_url,
                encoded,
                &known,
                (self.first_byte, self.idle),
                accounting.0,
                |decoded| apply(decoded, emit, text, &known),
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

/// Usage over the run's requests.
#[derive(Default)]
struct Meter {
    usage: Usage,
    /// Usage of every billed upstream request, for per-round pricing tiers.
    rounds: Vec<aether_support::TokenUsage>,
    context: Option<ContextUsage>,
    started: bool,
}

fn estimate_request(value: &serde_json::Value) -> u32 {
    fn string_chars(text: &str) -> usize {
        2 + text
            .chars()
            .map(|ch| match ch {
                '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
                '\u{00}'..='\u{1f}' => 6,
                _ => 1,
            })
            .sum::<usize>()
    }
    fn chars(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(map) => {
                let mut count = 2;
                let mut first = true;
                for (key, value) in map {
                    if matches!(key.as_str(), "images" | "image_url" | "data" | "source") {
                        continue;
                    }
                    count += usize::from(!first) + string_chars(key) + 1 + chars(value);
                    first = false;
                }
                count
            }
            serde_json::Value::Array(items) => {
                2 + items.len().saturating_sub(1) + items.iter().map(chars).sum::<usize>()
            }
            serde_json::Value::String(text) => string_chars(text),
            serde_json::Value::Number(number) => number.to_string().len(),
            serde_json::Value::Bool(value) => {
                if *value {
                    4
                } else {
                    5
                }
            }
            serde_json::Value::Null => 4,
        }
    }
    u32::try_from(chars(value).div_ceil(3)).unwrap_or(u32::MAX)
}

fn apply(
    decoded: Decoded,
    emit: &mut impl FnMut(RunEvent),
    text: &mut String,
    known: &[&str],
) -> Result<(), KitError> {
    match decoded {
        Decoded::Text(delta) => {
            text.push_str(&delta);
            emit(RunEvent::TextDelta { text: delta });
            Ok(())
        }
        Decoded::Reasoning {
            reasoning,
            text: delta,
        } => {
            emit(RunEvent::ReasoningDelta { text: reasoning });
            if let Some(delta) = delta {
                text.push_str(&delta);
                emit(RunEvent::TextDelta { text: delta });
            }
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
