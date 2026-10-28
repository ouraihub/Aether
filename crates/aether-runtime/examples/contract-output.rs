//! Emits real public Rust values for the language-independent schema check.

use std::io::{self, Write};

use aether_chat::{EndStatus, RunEvent, ToolOutcome, ToolSourceKind};
use aether_runtime::contract::conversation_dto::{
    UiConversation, UiConversationSummary, UiTurn, UiTurnStatus,
};
use aether_runtime::{RunAccepted, RunSnapshot, RunStatus};
use aether_support::{ContextSource, ContextUsage, KitError, TokenUsage};
use serde::Serialize;
use serde_json::{Value, json};

fn row<T: Serialize>(contract: &str, value: T) -> Result<Value, serde_json::Error> {
    Ok(json!({ "contract": contract, "value": serde_json::to_value(value)? }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = RunAccepted {
        run_id: "run-schema".into(),
        conversation_id: Some("conversation-schema".into()),
        turn_id: Some("turn-schema".into()),
    };
    let mut values = vec![row("RunAccepted", accepted)?];
    for status in [
        RunStatus::Running,
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::Cancelled,
    ] {
        let failed = status == RunStatus::Failed;
        values.push(row(
            "RunSnapshot",
            RunSnapshot {
                run_id: "run-schema".into(),
                conversation_id: None,
                turn_id: None,
                status,
                cancel_requested: status == RunStatus::Cancelled,
                error: failed.then(|| KitError::bad_request("input", "invalid")),
                exhausted: None,
            },
        )?);
    }
    let events = vec![
        RunEvent::SearchStart,
        RunEvent::SearchResult {
            query: "documentation".into(),
            provider: "local".into(),
            hits: vec![],
        },
        RunEvent::RoundStart { round: 1 },
        RunEvent::TextDelta {
            text: "你好".into(),
        },
        RunEvent::ReasoningDelta {
            text: "Inspecting the input".into(),
        },
        RunEvent::ToolCall {
            call_id: "call-schema".into(),
            name: "read_document".into(),
            source: ToolSourceKind::Host,
            server: None,
        },
        RunEvent::ApprovalRequired {
            call_id: "call-schema".into(),
            name: "read_document".into(),
            args_preview: "{}".into(),
        },
        RunEvent::ToolResult {
            call_id: "call-schema".into(),
            outcome: ToolOutcome::Ok,
            summary: "Read document".into(),
            payload: Some(json!({"found": true})),
        },
        RunEvent::Usage {
            input_tokens: Some(12),
            output_tokens: Some(3),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost: None,
        },
        RunEvent::Context {
            usage: ContextUsage {
                tokens: Some(15),
                context_window: Some(32000),
                source: ContextSource::Reported,
                endpoint_id: None,
                model_id: None,
            },
        },
        RunEvent::End {
            status: EndStatus::Completed,
            error: None,
            exhausted: Some(true),
        },
        RunEvent::End {
            status: EndStatus::Failed,
            error: Some(KitError::bad_request("input", "invalid")),
            exhausted: None,
        },
        RunEvent::End {
            status: EndStatus::Cancelled,
            error: None,
            exhausted: None,
        },
    ];
    for event in events {
        values.push(row("RunEvent", event)?);
    }
    values.push(row("KitError", KitError::bad_request("input", "invalid"))?);
    values.push(row(
        "Conversation",
        UiConversation {
            summary: UiConversationSummary {
                id: "conversation-schema".into(),
                title: "Contract example".into(),
                scope: None,
                selected_turn_id: Some("turn-schema".into()),
                created_at: 1,
                updated_at: 2,
            },
            turns: vec![UiTurn {
                id: "turn-schema".into(),
                parent_id: None,
                question_id: "question-schema".into(),
                supersedes_id: None,
                kind: "reply".into(),
                status: UiTurnStatus::Completed,
                input: "Hello".into(),
                text: "你好".into(),
                model_id: None,
                endpoint_id: None,
                error_code: None,
                usage: Some(TokenUsage {
                    input_tokens: Some(12),
                    output_tokens: Some(3),
                    ..TokenUsage::default()
                }),
                context_usage: None,
                reasoning_effort: None,
                cost: None,
                reasoning_text: None,
                images: vec![],
                generated: vec![],
            }],
        },
    )?);
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, &values)?;
    output.write_all(b"\n")?;
    Ok(())
}
