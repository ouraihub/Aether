//! Golden traces of the public run behaviour (docs/12 §5): every event, every request body the
//! upstream received, what the host saw and what the summary feeds back, per scenario and protocol.
//! Any change to the engine's observable behaviour changes `traces.json`; refactors must not.
//! Regenerate deliberately with `AETHER_WRITE_TRACES=1 cargo test -p aether-chat traces`.

use std::collections::BTreeMap;
use std::sync::{Mutex as StdMutex, PoisonError};

use aether_support::{Approval, Protocol};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::tools::{body, limited};
use super::{PROTOCOLS, after_first_delta, fixture, input, requests, runner, script, sse, sse_cut};
use crate::{
    Approver, AuthorizeContext, BoxFuture, PermissionMode, RunEvent, ToolCall, ToolEffect,
    ToolExecution, ToolExecutor, ToolSource, ToolSpec,
};
use aether_testkit::Reply;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/tests/traces.json");

/// Scripted host: what `authorize` answers, how the user answers, what tools return.
struct TraceHost {
    /// `None` passes the proposed approval through.
    approval: Option<Approval>,
    /// `None` waits until the run is cancelled.
    allow: Option<bool>,
    reply: ToolExecution,
    cancel_in_execute: Option<CancellationToken>,
    log: StdMutex<Vec<Value>>,
}

impl TraceHost {
    fn new() -> Self {
        Self {
            approval: None,
            allow: Some(true),
            reply: ToolExecution::Ok {
                text: "contents".to_owned(),
                payload: None,
            },
            cancel_in_execute: None,
            log: StdMutex::default(),
        }
    }
    fn record(&self, entry: Value) {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }
}

#[async_trait::async_trait]
impl ToolExecutor for TraceHost {
    fn authorize(&self, call: &ToolCall, ctx: &AuthorizeContext<'_>) -> Approval {
        self.record(json!({"authorize": call.id, "proposed": format!("{:?}", ctx.proposed)}));
        self.approval.unwrap_or(ctx.proposed)
    }
    async fn execute(&self, call: &ToolCall, _cancel: &CancellationToken) -> ToolExecution {
        self.record(json!({"execute": call.id}));
        if let Some(cancel) = &self.cancel_in_execute {
            cancel.cancel();
            return ToolExecution::Unknown {
                reason: "cancelled".to_owned(),
            };
        }
        self.reply.clone()
    }
}

impl Approver for TraceHost {
    fn wait(&self, _run: &str, call: &ToolCall, cancel: &CancellationToken) -> BoxFuture<'_, bool> {
        self.record(json!({"wait": call.id}));
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

/// When the test itself cancels the run.
#[derive(Clone, Copy, PartialEq)]
enum CancelOn {
    Never,
    Approval,
    FirstText,
}

struct Scenario {
    name: &'static str,
    replies: fn(Protocol) -> Vec<Reply>,
    tools: fn() -> Vec<ToolSpec>,
    host: fn(&CancellationToken) -> TraceHost,
    mode: PermissionMode,
    max_rounds: Option<u8>,
    cancel: CancelOn,
}

fn tool(approval: Approval, effect: ToolEffect, source: ToolSource) -> ToolSpec {
    ToolSpec {
        name: "read_file".to_owned(),
        description: "read_file tool".to_owned(),
        parameters: json!({ "type": "object" }),
        approval,
        effect,
        source,
    }
}
fn auto_read() -> Vec<ToolSpec> {
    vec![tool(Approval::Auto, ToolEffect::Read, ToolSource::Host)]
}
fn ask_read() -> Vec<ToolSpec> {
    vec![tool(Approval::Ask, ToolEffect::Read, ToolSource::Host)]
}
fn multi_then_text(protocol: Protocol) -> Vec<Reply> {
    vec![
        sse(fixture(protocol, "multi-tool")),
        sse(fixture(protocol, "text")),
    ]
}
fn plain(_: &CancellationToken) -> TraceHost {
    TraceHost::new()
}

fn scenarios() -> Vec<Scenario> {
    let base = |name, replies, tools, host| Scenario {
        name,
        replies,
        tools,
        host,
        mode: PermissionMode::Default,
        max_rounds: None,
        cancel: CancelOn::Never,
    };
    vec![
        base("text", |p| vec![sse(fixture(p, "text"))], auto_read, plain),
        base("auto_two_tools", multi_then_text, auto_read, plain),
        base("ask_allow", multi_then_text, ask_read, plain),
        base("ask_deny", multi_then_text, ask_read, |_| TraceHost {
            allow: Some(false),
            ..TraceHost::new()
        }),
        base("unknown_result", multi_then_text, auto_read, |_| {
            TraceHost {
                reply: ToolExecution::Unknown {
                    reason: "timeout".to_owned(),
                },
                ..TraceHost::new()
            }
        }),
        base("error_with_payload", multi_then_text, auto_read, |_| {
            TraceHost {
                reply: ToolExecution::Error {
                    code: "invalid".to_owned(),
                    message: "fix arguments".to_owned(),
                    payload: Some(json!({"diagnostic": "ui only"})),
                },
                ..TraceHost::new()
            }
        }),
        base(
            "repeated_call_ids",
            |p| {
                let multi = sse(fixture(p, "multi-tool"));
                vec![multi.clone(), multi, sse(fixture(p, "text"))]
            },
            auto_read,
            plain,
        ),
        base(
            "reused_id_changed_args",
            |p| {
                let changed = fixture(p, "multi-tool").replace("a.md", "changed.md");
                vec![sse(fixture(p, "multi-tool")), sse(changed)]
            },
            auto_read,
            plain,
        ),
        base(
            "truncated_round",
            |p| vec![sse(limited(p)), sse(fixture(p, "text"))],
            auto_read,
            plain,
        ),
        Scenario {
            max_rounds: Some(2),
            ..base(
                "round_limit",
                |p| vec![sse(fixture(p, "multi-tool"))],
                auto_read,
                plain,
            )
        },
        Scenario {
            cancel: CancelOn::Approval,
            ..base("cancel_at_approval", multi_then_text, ask_read, |_| {
                TraceHost {
                    allow: None,
                    ..TraceHost::new()
                }
            })
        },
        base("cancel_in_tool", multi_then_text, auto_read, |cancel| {
            TraceHost {
                cancel_in_execute: Some(cancel.clone()),
                ..TraceHost::new()
            }
        }),
        Scenario {
            cancel: CancelOn::FirstText,
            ..base(
                "cancel_mid_stream",
                |p| {
                    let body = fixture(p, "text");
                    vec![sse_cut(&body, after_first_delta(&body))]
                },
                auto_read,
                plain,
            )
        },
        base(
            "in_stream_error",
            |p| vec![sse(fixture(p, "upstream-error"))],
            auto_read,
            plain,
        ),
        base(
            "status_500",
            |_| vec![Reply::status(500, r#"{"error":{"message":"boom"}}"#)],
            auto_read,
            plain,
        ),
        base(
            "tools_rejected_retry",
            |p| {
                let refusal = r#"{"error":{"message":"tools are not supported"}}"#;
                vec![Reply::status(400, refusal), sse(fixture(p, "text"))]
            },
            auto_read,
            plain,
        ),
        Scenario {
            mode: PermissionMode::Plan,
            ..base(
                "plan_hides_edit_tool",
                multi_then_text,
                || vec![tool(Approval::Auto, ToolEffect::Edit, ToolSource::Host)],
                plain,
            )
        },
        base(
            "catalogue_deny",
            multi_then_text,
            || vec![tool(Approval::Deny, ToolEffect::Read, ToolSource::Host)],
            |_| TraceHost {
                approval: Some(Approval::Auto),
                ..TraceHost::new()
            },
        ),
        Scenario {
            mode: PermissionMode::FullAccess,
            ..base(
                "mcp_tool_full_access",
                multi_then_text,
                || {
                    let server = ToolSource::Mcp {
                        server: "docs".to_owned(),
                    };
                    vec![tool(Approval::Ask, ToolEffect::Other, server)]
                },
                plain,
            )
        },
    ]
}

/// Replaces per-run random markers and log-correlation ids so traces compare exactly.
fn normalize(value: &mut Value) {
    match value {
        Value::String(text) => {
            for marker in ["TOOL_RESULT_", "SKILL_"] {
                let mut out = String::with_capacity(text.len());
                let mut rest = text.as_str();
                while let Some(at) = rest.find(marker) {
                    let end = at + marker.len();
                    out.push_str(&rest[..end]);
                    let id = &rest[end..];
                    if id.len() >= 12 && id[..12].bytes().all(|b| b.is_ascii_hexdigit()) {
                        out.push_str("<boundary>");
                        rest = &id[12..];
                    } else {
                        rest = id;
                    }
                }
                out.push_str(rest);
                *text = out;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize),
        Value::Object(map) => {
            if map.contains_key("errorId") {
                map.insert("errorId".to_owned(), json!("<id>"));
            }
            map.values_mut().for_each(normalize);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

async fn trace(scenario: &Scenario, protocol: Protocol) -> Value {
    let (base, server) = script((scenario.replies)(protocol)).await;
    let mut input = input(protocol, base);
    input.history.clear();
    input.permission_mode = scenario.mode;
    input.tools = (scenario.tools)();
    if let Some(max) = scenario.max_rounds {
        input.budget.max_rounds = max;
    }
    let cancel = CancellationToken::new();
    let host = (scenario.host)(&cancel);
    let trigger = cancel.clone();
    let mut events = Vec::new();
    let summary = runner(5_000, 5_000)
        .run(input, &host, &host, &cancel, |event| {
            let stop = match scenario.cancel {
                CancelOn::Approval => matches!(event, RunEvent::ApprovalRequired { .. }),
                CancelOn::FirstText => matches!(event, RunEvent::TextDelta { .. }),
                CancelOn::Never => false,
            };
            if stop {
                trigger.cancel();
            }
            // Context accounting has dedicated report/estimate/failure assertions.
            if !matches!(event, RunEvent::Context { .. }) {
                events.push(serde_json::to_value(&event).unwrap());
            }
        })
        .await;
    let requests: Vec<Value> = requests(&server).iter().map(|raw| body(raw)).collect();
    let mut value = json!({
        "events": events,
        "host": host.log.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        "requests": requests,
        "summary": {
            "status": summary.status,
            "exhausted": summary.exhausted,
            "messages": summary.messages,
        },
    });
    normalize(&mut value);
    value
}

#[tokio::test]
async fn public_run_behaviour_matches_the_golden_traces() {
    let mut actual = BTreeMap::new();
    for scenario in scenarios() {
        for protocol in PROTOCOLS {
            actual.insert(
                format!("{}/{protocol:?}", scenario.name),
                trace(&scenario, protocol).await,
            );
        }
    }
    let actual = Value::Object(actual.into_iter().collect());
    if std::env::var_os("AETHER_WRITE_TRACES").is_some() {
        std::fs::write(
            GOLDEN,
            serde_json::to_string_pretty(&actual).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(GOLDEN).unwrap()).unwrap();
    if golden != actual {
        // Full actual traces for diffing against the golden file.
        let dump = std::env::temp_dir().join("aether-chat-traces.actual.json");
        std::fs::write(&dump, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        for (key, want) in golden.as_object().unwrap() {
            assert_eq!(
                actual.get(key),
                Some(want),
                "trace {key} changed; actual in {}",
                dump.display()
            );
        }
    }
    assert_eq!(
        actual.as_object().unwrap().len(),
        golden.as_object().unwrap().len()
    );
}
