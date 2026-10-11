//! The tool loop against a loopback upstream scripted per round (docs/12 §5).

use std::sync::{Mutex as StdMutex, PoisonError};

use aether_support::{Approval, Protocol};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::tools::{expected, limited};
use super::{PROTOCOLS, Reply, end, fixture, input, runner, script, sse};
use crate::tools::{DENIED_NOTE, TRUNCATED_NOTE, UNKNOWN_NOTE};
use crate::{
    Approver, BoxFuture, EndStatus, Message, RunEvent, RunInput, RunSummary, ToolCall,
    ToolExecution, ToolExecutor, ToolOutcome, ToolSource, ToolSpec,
};

/// Fake host: records executions and approval requests.
struct Host {
    approval: Approval,
    /// Answer of the approver; `None` waits until the run is cancelled.
    allow: Option<bool>,
    reply: ToolExecution,
    /// Cancel the run from inside `execute`, then return `Unknown` like a cut-off tool.
    cancel_in_execute: Option<CancellationToken>,
    executed: StdMutex<Vec<String>>,
    asked: StdMutex<Vec<String>>,
}

impl Host {
    fn new(approval: Approval) -> Self {
        Self {
            approval,
            allow: Some(true),
            reply: ToolExecution::Ok {
                text: "contents".to_owned(),
                payload: None,
            },
            cancel_in_execute: None,
            executed: StdMutex::default(),
            asked: StdMutex::default(),
        }
    }
    fn executed(&self) -> Vec<String> {
        log(&self.executed).clone()
    }
}

/// A poisoned log still holds what was recorded before the panic.
fn log(list: &StdMutex<Vec<String>>) -> std::sync::MutexGuard<'_, Vec<String>> {
    list.lock().unwrap_or_else(PoisonError::into_inner)
}

#[async_trait::async_trait]
impl ToolExecutor for Host {
    fn authorize(&self, _call: &ToolCall, default: Approval) -> Approval {
        assert_eq!(
            default,
            Approval::Auto,
            "catalogue default is passed through"
        );
        self.approval
    }
    async fn execute(&self, call: &ToolCall, _cancel: &CancellationToken) -> ToolExecution {
        log(&self.executed).push(call.id.clone());
        if let Some(cancel) = &self.cancel_in_execute {
            cancel.cancel();
            return ToolExecution::Unknown {
                reason: "cancelled".to_owned(),
            };
        }
        self.reply.clone()
    }
}

impl Approver for Host {
    fn wait(&self, _run: &str, call: &ToolCall, cancel: &CancellationToken) -> BoxFuture<'_, bool> {
        log(&self.asked).push(call.id.clone());
        let allow = self.allow;
        let cancel = cancel.clone();
        Box::pin(async move {
            match allow {
                Some(allow) => allow,
                None => {
                    cancel.cancelled().await;
                    false
                }
            }
        })
    }
}

fn read_file() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "read_file".to_owned(),
        description: "Reads a file".to_owned(),
        parameters: json!({ "type": "object" }),
        approval: Approval::Auto,
        source: ToolSource::Host,
    }]
}

/// Input whose upstream answers `replies` in order, with the `read_file` tool.
async fn scripted(protocol: Protocol, replies: Vec<Reply>) -> (RunInput, super::Seen) {
    let (base, seen) = script(replies).await;
    let mut input = input(protocol, base);
    input.history.clear();
    input.tools = read_file();
    (input, seen)
}

/// Runs with `host`; `cancel_on_approval` cancels once approval is requested.
async fn run_with(
    host: &Host,
    input: RunInput,
    cancel: &CancellationToken,
    cancel_on_approval: bool,
) -> (RunSummary, Vec<RunEvent>) {
    let mut events = Vec::new();
    let summary = runner(5_000, 5_000)
        .run(input, host, host, cancel, |event| {
            if cancel_on_approval && matches!(event, RunEvent::ApprovalRequired { .. }) {
                cancel.cancel();
            }
            events.push(event);
        })
        .await;
    (summary, events)
}

fn outcomes(events: &[RunEvent]) -> Vec<(String, ToolOutcome)> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolResult {
                call_id, outcome, ..
            } => Some((call_id.clone(), *outcome)),
            _ => None,
        })
        .collect()
}

fn fed_back(summary: &RunSummary) -> Vec<(&str, &str)> {
    summary
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::Tool { call_id, text } => Some((call_id.as_str(), text.as_str())),
            _ => None,
        })
        .collect()
}

fn ids(protocol: Protocol) -> (String, String) {
    let calls = expected(protocol, "multi-tool");
    (calls[0].id.clone(), calls[1].id.clone())
}

fn multi(protocol: Protocol) -> Reply {
    sse(fixture(protocol, "multi-tool"))
}

fn text(protocol: Protocol) -> Reply {
    sse(fixture(protocol, "text"))
}

#[tokio::test]
async fn truncated_round_executes_nothing_and_says_why() {
    for protocol in PROTOCOLS {
        let (input, seen) = scripted(protocol, vec![sse(limited(protocol)), text(protocol)]).await;
        let host = Host::new(Approval::Auto);
        let (_, events) = run_with(&host, input, &CancellationToken::new(), false).await;
        assert!(host.executed().is_empty(), "{protocol:?}");
        let seen = seen.lock().await;
        assert_eq!(seen.len(), 2, "{protocol:?}");
        assert!(seen[1].contains(TRUNCATED_NOTE), "{protocol:?}");
        assert_eq!(end(&events).0, EndStatus::Completed);
    }
}

#[tokio::test]
async fn ask_waits_for_the_user_and_deny_feeds_back_denied() {
    for protocol in PROTOCOLS {
        let (a, b) = ids(protocol);
        for allow in [true, false] {
            let (input, _) = scripted(protocol, vec![multi(protocol), text(protocol)]).await;
            let mut host = Host::new(Approval::Ask);
            host.allow = Some(allow);
            let (summary, events) = run_with(&host, input, &CancellationToken::new(), false).await;
            assert_eq!(*log(&host.asked), [a.clone(), b.clone()], "{protocol:?}");
            let want = if allow {
                ToolOutcome::Ok
            } else {
                ToolOutcome::Denied
            };
            assert_eq!(outcomes(&events), [(a.clone(), want), (b.clone(), want)]);
            if allow {
                assert_eq!(host.executed(), [a.clone(), b.clone()]);
            } else {
                assert!(host.executed().is_empty(), "{protocol:?}");
                assert!(fed_back(&summary).iter().all(|(_, t)| *t == DENIED_NOTE));
            }
            assert_eq!(end(&events).0, EndStatus::Completed);
        }
    }
}

#[tokio::test]
async fn unknown_outcome_is_fed_back_and_never_retried() {
    for protocol in PROTOCOLS {
        let (a, b) = ids(protocol);
        let (input, seen) = scripted(protocol, vec![multi(protocol), text(protocol)]).await;
        let mut host = Host::new(Approval::Auto);
        host.reply = ToolExecution::Unknown {
            reason: "timeout".to_owned(),
        };
        let (summary, events) = run_with(&host, input, &CancellationToken::new(), false).await;
        assert_eq!(host.executed(), [a.clone(), b.clone()], "{protocol:?}");
        assert_eq!(
            outcomes(&events),
            [(a, ToolOutcome::Unknown), (b, ToolOutcome::Unknown)]
        );
        assert!(fed_back(&summary).iter().all(|(_, t)| *t == UNKNOWN_NOTE));
        assert_eq!(seen.lock().await.len(), 2, "{protocol:?}");
    }
}

#[tokio::test]
async fn repeated_call_id_executes_once() {
    for protocol in PROTOCOLS {
        let (a, b) = ids(protocol);
        let replies = vec![multi(protocol), multi(protocol), text(protocol)];
        let (input, _) = scripted(protocol, replies).await;
        let host = Host::new(Approval::Auto);
        let (summary, events) = run_with(&host, input, &CancellationToken::new(), false).await;
        assert_eq!(host.executed(), [a.clone(), b.clone()], "{protocol:?}");
        assert_eq!(outcomes(&events).len(), 2, "{protocol:?}");
        // Both rounds still get a result for every call.
        let fed: Vec<_> = fed_back(&summary)
            .iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(fed, [a.clone(), b.clone(), a, b]);
    }
}

#[tokio::test]
async fn round_limit_stops_the_loop_as_exhausted() {
    for protocol in PROTOCOLS {
        // Every request asks for tools again.
        let (mut input, seen) = scripted(protocol, vec![multi(protocol)]).await;
        input.budget.max_rounds = 2;
        let host = Host::new(Approval::Auto);
        let (summary, events) = run_with(&host, input, &CancellationToken::new(), false).await;
        assert_eq!(seen.lock().await.len(), 2, "{protocol:?}");
        assert!(summary.exhausted, "{protocol:?}");
        assert_eq!(end(&events).0, EndStatus::Completed);
        assert!(matches!(
            events.last(),
            Some(RunEvent::End {
                exhausted: Some(true),
                ..
            })
        ));
    }
}

#[tokio::test]
async fn cancel_while_awaiting_approval_starts_nothing() {
    for protocol in PROTOCOLS {
        let (input, seen) = scripted(protocol, vec![multi(protocol), text(protocol)]).await;
        let mut host = Host::new(Approval::Ask);
        host.allow = None;
        let (_, events) = run_with(&host, input, &CancellationToken::new(), true).await;
        assert!(host.executed().is_empty(), "{protocol:?}");
        assert_eq!(log(&host.asked).len(), 1, "{protocol:?}");
        assert_eq!(seen.lock().await.len(), 1, "{protocol:?}");
        assert_eq!(end(&events).0, EndStatus::Cancelled);
    }
}

#[tokio::test]
async fn cancel_during_a_tool_starts_no_further_tool() {
    for protocol in PROTOCOLS {
        let (a, _) = ids(protocol);
        let (input, seen) = scripted(protocol, vec![multi(protocol), text(protocol)]).await;
        let cancel = CancellationToken::new();
        let mut host = Host::new(Approval::Auto);
        host.cancel_in_execute = Some(cancel.clone());
        let (_, events) = run_with(&host, input, &cancel, false).await;
        assert_eq!(host.executed(), [a], "{protocol:?}");
        assert_eq!(seen.lock().await.len(), 1, "{protocol:?}");
        assert_eq!(end(&events).0, EndStatus::Cancelled);
    }
}
