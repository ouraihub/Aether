//! The tool loop against a loopback upstream scripted per round (docs/12 §5).

use std::sync::{Mutex as StdMutex, PoisonError};

use aether_support::{Approval, Protocol};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::tools::{expected, limited};
use super::{PROTOCOLS, Reply, end, fixture, input, runner, script, sse};
use crate::tools::{DENIED_NOTE, TRUNCATED_NOTE, UNKNOWN_NOTE, proposed};
use crate::validate::PLAN_NOTICE;
use crate::{
    Approver, AuthorizeContext, BoxFuture, EndStatus, Message, PermissionMode, RunEvent, RunInput,
    RunSummary, ToolCall, ToolEffect, ToolExecution, ToolExecutor, ToolOutcome, ToolSource,
    ToolSpec,
};

/// What one `authorize` call saw.
#[derive(Debug, Clone, PartialEq)]
struct Authorized {
    mode: PermissionMode,
    purpose: String,
    run_id: String,
    effect: ToolEffect,
    source: ToolSource,
    proposed: Approval,
}

/// Fake host: records executions and approval requests.
struct Host {
    /// Answer of `authorize`; `None` passes `ctx.proposed` through, like the default host.
    approval: Option<Approval>,
    /// Answer of the approver; `None` waits until the run is cancelled.
    allow: Option<bool>,
    reply: ToolExecution,
    /// Cancel the run from inside `execute`, then return `Unknown` like a cut-off tool.
    cancel_in_execute: Option<CancellationToken>,
    executed: StdMutex<Vec<String>>,
    asked: StdMutex<Vec<String>>,
    authorized: StdMutex<Vec<Authorized>>,
}

impl Host {
    fn new(approval: Approval) -> Self {
        Self {
            approval: Some(approval),
            ..Self::proposing()
        }
    }
    /// Answers `authorize` with the proposed approval.
    fn proposing() -> Self {
        Self {
            approval: None,
            allow: Some(true),
            reply: ToolExecution::Ok {
                text: "contents".to_owned(),
                payload: None,
            },
            cancel_in_execute: None,
            executed: StdMutex::default(),
            asked: StdMutex::default(),
            authorized: StdMutex::default(),
        }
    }
    fn executed(&self) -> Vec<String> {
        log(&self.executed).clone()
    }
    fn authorized(&self) -> Vec<Authorized> {
        self.authorized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A poisoned log still holds what was recorded before the panic.
fn log(list: &StdMutex<Vec<String>>) -> std::sync::MutexGuard<'_, Vec<String>> {
    list.lock().unwrap_or_else(PoisonError::into_inner)
}

#[async_trait::async_trait]
impl ToolExecutor for Host {
    fn authorize(&self, _call: &ToolCall, ctx: &AuthorizeContext<'_>) -> Approval {
        self.authorized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Authorized {
                mode: ctx.mode,
                purpose: ctx.purpose.to_owned(),
                run_id: ctx.run_id.to_owned(),
                effect: ctx.effect,
                source: ctx.source.clone(),
                proposed: ctx.proposed,
            });
        self.approval.unwrap_or(ctx.proposed)
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
    vec![spec("read_file", Approval::Auto, ToolEffect::Read)]
}

fn spec(name: &str, approval: Approval, effect: ToolEffect) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: format!("{name} tool"),
        parameters: json!({ "type": "object" }),
        approval,
        effect,
        source: ToolSource::Host,
    }
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

const MODES: [PermissionMode; 5] = [
    PermissionMode::Default,
    PermissionMode::Plan,
    PermissionMode::AcceptEdits,
    PermissionMode::Auto,
    PermissionMode::FullAccess,
];
const EFFECTS: [ToolEffect; 4] = [
    ToolEffect::Read,
    ToolEffect::Edit,
    ToolEffect::Exec,
    ToolEffect::Other,
];

#[test]
fn mode_matrix_relaxes_only_ask_and_never_deny() {
    use PermissionMode::{AcceptEdits, Auto, Default, FullAccess, Plan};
    let mcp = ToolSource::Mcp {
        server: "docs".to_owned(),
    };
    for mode in MODES {
        for effect in EFFECTS {
            for source in [ToolSource::Host, mcp.clone()] {
                let tool = |approval| ToolSpec {
                    source: source.clone(),
                    ..spec("t", approval, effect)
                };
                // `Auto` and `Deny` are never changed by a mode.
                assert_eq!(proposed(mode, &tool(Approval::Auto)), Approval::Auto);
                assert_eq!(proposed(mode, &tool(Approval::Deny)), Approval::Deny);
                let relaxed = match mode {
                    Default | Plan => false,
                    AcceptEdits => effect == ToolEffect::Edit,
                    Auto => source == ToolSource::Host,
                    FullAccess => true,
                };
                let want = if relaxed {
                    Approval::Auto
                } else {
                    Approval::Ask
                };
                assert_eq!(
                    proposed(mode, &tool(Approval::Ask)),
                    want,
                    "{mode:?} {effect:?} {source:?}"
                );
            }
        }
    }
}

/// The tool names and system text of the first request of a run in `mode`.
async fn first_request(protocol: Protocol, mode: PermissionMode) -> (Vec<String>, String) {
    let (mut input, seen) = scripted(protocol, vec![text(protocol)]).await;
    input.permission_mode = mode;
    input.tools = EFFECTS
        .iter()
        .map(|&effect| spec(&format!("{effect:?}_tool"), Approval::Ask, effect))
        .collect();
    let host = Host::proposing();
    let (_, events) = run_with(&host, input, &CancellationToken::new(), false).await;
    assert_eq!(end(&events).0, EndStatus::Completed);
    let raw = seen.lock().await[0].clone();
    let body = super::request_body(&raw);
    let names = body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    tool.pointer("/function/name")
                        .or_else(|| tool.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap()
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default();
    (names, raw)
}

#[tokio::test]
async fn plan_offers_only_read_tools_and_says_so() {
    for protocol in PROTOCOLS {
        for mode in MODES {
            let (names, raw) = first_request(protocol, mode).await;
            if mode == PermissionMode::Plan {
                assert_eq!(names, ["Read_tool"], "{protocol:?}");
                assert!(raw.contains(PLAN_NOTICE), "{protocol:?}");
            } else {
                assert_eq!(
                    names,
                    ["Read_tool", "Edit_tool", "Exec_tool", "Other_tool"],
                    "{protocol:?} {mode:?}"
                );
                assert!(!raw.contains(PLAN_NOTICE), "{protocol:?} {mode:?}");
            }
        }
    }
}

/// `read_file` with `approval` and `effect`, asked for twice by the multi-tool fixture.
async fn mode_run(
    protocol: Protocol,
    mode: PermissionMode,
    host: &Host,
    tool: ToolSpec,
) -> Vec<RunEvent> {
    let (mut input, _) = scripted(protocol, vec![multi(protocol), text(protocol)]).await;
    input.permission_mode = mode;
    input.purpose = "notes".to_owned();
    input.tools = vec![tool];
    run_with(host, input, &CancellationToken::new(), false)
        .await
        .1
}

#[tokio::test]
async fn authorize_sees_the_mode_and_its_answer_is_final() {
    let protocol = Protocol::OpenaiChat;
    let (a, b) = ids(protocol);
    let mcp = ToolSource::Mcp {
        server: "docs".to_owned(),
    };
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Auto,
        PermissionMode::FullAccess,
    ] {
        for (effect, source) in [
            (ToolEffect::Edit, ToolSource::Host),
            (ToolEffect::Exec, ToolSource::Host),
            (ToolEffect::Other, mcp.clone()),
        ] {
            let tool = ToolSpec {
                source: source.clone(),
                ..spec("read_file", Approval::Ask, effect)
            };
            let want = proposed(mode, &tool);
            let host = Host::proposing();
            let events = mode_run(protocol, mode, &host, tool).await;
            let seen = host.authorized();
            assert_eq!(seen.len(), 2, "{mode:?} {effect:?}");
            assert_eq!(
                seen[0],
                Authorized {
                    mode,
                    purpose: "notes".to_owned(),
                    run_id: "r1".to_owned(),
                    effect,
                    source: source.clone(),
                    proposed: want,
                }
            );
            // A proposed `Auto` runs without asking; `Ask` waits for the user.
            let asked = log(&host.asked).len();
            assert_eq!(asked, if want == Approval::Ask { 2 } else { 0 });
            assert_eq!(host.executed(), [a.clone(), b.clone()]);
            assert_eq!(outcomes(&events).len(), 2);
        }
    }

    // The host turns a proposed `Auto` back into `Ask` (rules) or `Deny`.
    let tool = spec("read_file", Approval::Ask, ToolEffect::Exec);
    let mut host = Host::new(Approval::Ask);
    host.allow = Some(false);
    let events = mode_run(protocol, PermissionMode::FullAccess, &host, tool.clone()).await;
    assert_eq!(host.authorized()[0].proposed, Approval::Auto);
    assert_eq!(log(&host.asked).len(), 2);
    assert!(host.executed().is_empty());
    assert_eq!(outcomes(&events)[0].1, ToolOutcome::Denied);
    let host = Host::new(Approval::Deny);
    let events = mode_run(protocol, PermissionMode::FullAccess, &host, tool).await;
    assert!(log(&host.asked).is_empty());
    assert!(host.executed().is_empty());
    assert_eq!(outcomes(&events)[1].1, ToolOutcome::Denied);
}

#[tokio::test]
async fn catalogue_deny_is_never_relaxed() {
    let protocol = Protocol::OpenaiChat;
    for mode in MODES {
        // Even a host answering `Auto` to everything cannot run a denied tool.
        let host = Host::new(Approval::Auto);
        let tool = spec("read_file", Approval::Deny, ToolEffect::Read);
        let events = mode_run(protocol, mode, &host, tool).await;
        assert!(host.executed().is_empty(), "{mode:?}");
        assert!(host.authorized().is_empty(), "{mode:?}");
        assert!(log(&host.asked).is_empty(), "{mode:?}");
        assert!(
            outcomes(&events)
                .iter()
                .all(|(_, outcome)| *outcome == ToolOutcome::Denied),
            "{mode:?}"
        );
    }
}

#[tokio::test]
async fn plan_call_to_a_hidden_tool_is_unknown_and_never_runs() {
    let protocol = Protocol::OpenaiChat;
    let (input, seen) = scripted(protocol, vec![multi(protocol), text(protocol)]).await;
    let mut input = input;
    input.permission_mode = PermissionMode::Plan;
    // The model names `read_file`, which is an edit tool here and so not in the Plan catalogue.
    input.tools = vec![spec("read_file", Approval::Auto, ToolEffect::Edit)];
    let host = Host::proposing();
    let (summary, _) = run_with(&host, input, &CancellationToken::new(), false).await;
    assert!(host.executed().is_empty());
    assert!(host.authorized().is_empty());
    assert!(
        fed_back(&summary)
            .iter()
            .all(|(_, text)| text.contains("unknown_tool"))
    );
    assert_eq!(seen.lock().await.len(), 2);
}
