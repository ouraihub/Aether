//! The decisions of one run as a state machine (docs/12 §5): rounds and the round limit,
//! truncation, per-run call-id dedup, catalogue and approval routing, and the cancellation
//! checkpoints. It performs no I/O and reads no clock: the driver in `run.rs` performs each
//! [`Action`] and hands the outcome back, so the decisions do not depend on how I/O is done.

use std::collections::{HashMap, VecDeque};
use std::mem;

use aether_support::{KitError, KitErrorCode};
use tokio_util::sync::CancellationToken;

use crate::protocol::Round;
use crate::tools::{self, AuthorizeContext, Settled, TRUNCATED_NOTE};
use crate::types::{
    Approval, Message, RunEvent, RunInput, ToolCall, ToolExecution, ToolSource, ToolSourceKind,
    ToolSpec,
};

/// A catalogued call between its `tool_call` event and its result; owned by the action that
/// works on it and handed back with the outcome.
pub(crate) struct Pending {
    pub(crate) call: ToolCall,
    spec: usize,
}

/// What the driver performs next.
pub(crate) enum Action {
    /// One model request with [`Machine::catalogue`] over [`Machine::messages`], streaming
    /// text into the driver's buffer; answer with [`Machine::model_done`].
    Model,
    /// Ask the host ([`Machine::authorize_context`]); answer with [`Machine::authorized`].
    Authorize(Pending),
    /// Register the approver wait, then emit the event, then wait; answer with
    /// [`Machine::approved`]. Registering first means a fast answer cannot be missed.
    Approve(Pending, RunEvent),
    /// Execute the call; answer with [`Machine::executed`].
    Execute(Pending),
    /// The loop ended: `Ok(exhausted)` or the error that ended it.
    Done(Result<bool, KitError>),
}

/// Settled call: name, arguments and the text fed back, reused for a repeated call id.
type Known = (String, serde_json::Value, String);

pub(crate) struct Machine<'a> {
    input: &'a RunInput,
    cancel: &'a CancellationToken,
    boundary: String,
    catalogue: &'a [ToolSpec],
    round: u8,
    messages: Vec<Message>,
    calls: VecDeque<ToolCall>,
    settled: HashMap<String, Known>,
}

impl<'a> Machine<'a> {
    /// `boundary` marks external tool results for this run; `cancel` is only read at checkpoints.
    pub(crate) fn new(
        input: &'a RunInput,
        boundary: String,
        cancel: &'a CancellationToken,
    ) -> Self {
        Self {
            input,
            cancel,
            boundary,
            catalogue: &input.tools,
            round: 0,
            messages: Vec::new(),
            calls: VecDeque::new(),
            settled: HashMap::new(),
        }
    }

    pub(crate) fn catalogue(&self) -> &'a [ToolSpec] {
        self.catalogue
    }
    pub(crate) fn messages(&self) -> &[Message] {
        &self.messages
    }
    pub(crate) fn into_messages(self) -> Vec<Message> {
        self.messages
    }

    pub(crate) fn start(&mut self, emit: &mut impl FnMut(RunEvent)) -> Action {
        self.next_round(emit)
    }

    /// No new request after a cancel (docs/12 §5 step 7).
    fn next_round(&mut self, emit: &mut impl FnMut(RunEvent)) -> Action {
        if self.round >= self.input.budget.max_rounds {
            return Action::Done(Ok(true));
        }
        self.round += 1;
        if self.cancel.is_cancelled() {
            return Action::Done(Err(KitError::cancelled()));
        }
        emit(RunEvent::RoundStart { round: self.round });
        Action::Model
    }

    /// `text` is what this request streamed; the machine takes it into the turn.
    pub(crate) fn model_done(
        &mut self,
        result: Result<Round, KitError>,
        text: &mut String,
        emit: &mut impl FnMut(RunEvent),
    ) -> Action {
        let round = match result {
            // At most once per round: the retry runs with an empty catalogue.
            Err(error)
                if rejects_tools(&error)
                    && !self.catalogue.is_empty()
                    && self.input.connection.supports_tools.is_none()
                    && text.is_empty() =>
            {
                tracing::warn!(run_id = %self.input.run_id, "chat.run.tools_rejected_retry");
                self.catalogue = &[];
                return Action::Model;
            }
            Err(error) => return Action::Done(Err(error)),
            Ok(round) => round,
        };
        if round.tool_calls.is_empty() || round.truncated {
            if !text.is_empty() || !round.reasoning.is_empty() {
                self.messages.push(Message::Assistant {
                    text: mem::take(text),
                    tool_calls: Vec::new(),
                    reasoning: round.reasoning,
                });
            }
            if round.tool_calls.is_empty() {
                return Action::Done(Ok(false));
            }
            // Arguments may be cut off: execute none of them (docs/12 §8 trap 1).
            self.messages.push(Message::User(TRUNCATED_NOTE.to_owned()));
            return self.next_round(emit);
        }
        self.messages.push(Message::Assistant {
            text: mem::take(text),
            tool_calls: round.tool_calls.clone(),
            reasoning: round.reasoning,
        });
        self.calls = round.tool_calls.into();
        self.next_call(emit)
    }

    /// Settles calls that need no host until one does, or the round's calls are done.
    fn next_call(&mut self, emit: &mut impl FnMut(RunEvent)) -> Action {
        loop {
            if self.cancel.is_cancelled() {
                return Action::Done(Err(KitError::cancelled()));
            }
            let Some(call) = self.calls.pop_front() else {
                return self.next_round(emit);
            };
            // Fed back again for a repeated call id instead of executing twice (docs/12 §5).
            if let Some((name, args, text)) = self.settled.get(&call.id) {
                if name != &call.name || args != &call.args {
                    return Action::Done(Err(KitError::new(
                        KitErrorCode::Conflict,
                        "tool call id reused with different arguments",
                    )
                    .with_reason("tool_call_conflict")));
                }
                let text = text.clone();
                self.messages.push(Message::Tool {
                    call_id: call.id,
                    text,
                });
                continue;
            }
            let Some(spec) = self
                .catalogue
                .iter()
                .position(|spec| spec.name == call.name)
            else {
                let text = tools::unknown_tool(&call);
                self.record(call, text);
                continue;
            };
            let (source, server) = match &self.catalogue[spec].source {
                ToolSource::Host => (ToolSourceKind::Host, None),
                ToolSource::Mcp { server } => (ToolSourceKind::Mcp, Some(server.clone())),
            };
            emit(RunEvent::ToolCall {
                call_id: call.id.clone(),
                name: call.name.clone(),
                source,
                server,
            });
            // A catalogue `Deny` is final: no mode and no host answer turns it into a run.
            if self.catalogue[spec].approval == Approval::Deny {
                self.settle(call, Settled::denied(), emit);
                continue;
            }
            return Action::Authorize(Pending { call, spec });
        }
    }

    pub(crate) fn authorize_context(&self, pending: &Pending) -> AuthorizeContext<'a> {
        let input = self.input;
        let spec = &self.catalogue[pending.spec];
        AuthorizeContext {
            mode: input.permission_mode,
            purpose: &input.purpose,
            run_id: &input.run_id,
            effect: spec.effect,
            source: &spec.source,
            proposed: tools::proposed(input.permission_mode, spec),
        }
    }

    pub(crate) fn authorized(
        &mut self,
        pending: Pending,
        decision: Approval,
        emit: &mut impl FnMut(RunEvent),
    ) -> Action {
        match decision {
            Approval::Auto => self.execute_unless_cancelled(pending),
            Approval::Deny => self.deny(pending, emit),
            Approval::Ask => {
                let event = RunEvent::ApprovalRequired {
                    call_id: pending.call.id.clone(),
                    name: pending.call.name.clone(),
                    args_preview: tools::args_preview(&pending.call),
                };
                Action::Approve(pending, event)
            }
        }
    }

    /// `None` when the run was cancelled while waiting.
    pub(crate) fn approved(
        &mut self,
        pending: Pending,
        allow: Option<bool>,
        emit: &mut impl FnMut(RunEvent),
    ) -> Action {
        match allow {
            None => Action::Done(Err(KitError::cancelled())),
            Some(true) => self.execute_unless_cancelled(pending),
            Some(false) => self.deny(pending, emit),
        }
    }

    /// An in-flight tool was awaited even after a cancel; it reports `unknown` itself.
    pub(crate) fn executed(
        &mut self,
        pending: Pending,
        execution: ToolExecution,
        emit: &mut impl FnMut(RunEvent),
    ) -> Action {
        let settled = Settled::from_execution(
            execution,
            &self.catalogue[pending.spec],
            self.input.budget.tool_output_chars,
            &self.boundary,
        );
        self.settle(pending.call, settled, emit);
        self.next_call(emit)
    }

    fn execute_unless_cancelled(&self, pending: Pending) -> Action {
        if self.cancel.is_cancelled() {
            Action::Done(Err(KitError::cancelled()))
        } else {
            Action::Execute(pending)
        }
    }

    fn deny(&mut self, pending: Pending, emit: &mut impl FnMut(RunEvent)) -> Action {
        self.settle(pending.call, Settled::denied(), emit);
        self.next_call(emit)
    }

    fn settle(&mut self, call: ToolCall, settled: Settled, emit: &mut impl FnMut(RunEvent)) {
        emit(RunEvent::ToolResult {
            call_id: call.id.clone(),
            outcome: settled.outcome,
            summary: settled.summary,
            payload: settled.payload,
        });
        self.record(call, settled.text);
    }

    fn record(&mut self, call: ToolCall, text: String) {
        self.messages.push(Message::Tool {
            call_id: call.id.clone(),
            text: text.clone(),
        });
        self.settled.insert(call.id, (call.name, call.args, text));
    }

    /// Keeps streamed text, including the partial text of a failed or cancelled round.
    pub(crate) fn flush(&mut self, text: &mut String) {
        if !text.is_empty() {
            self.messages.push(Message::Assistant {
                text: mem::take(text),
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
