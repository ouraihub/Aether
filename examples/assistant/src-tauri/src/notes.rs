//! Host tools of the example (docs/90 CH4-3): `read_note` (read-only, auto) and
//! `write_note` (edits, ask) over one in-memory note. All five permission modes are offered.

use std::sync::{Mutex, PoisonError};

use serde_json::json;
use tauri_plugin_aether::{
    Approval, CancellationToken, HostTool, KitHost, PermissionMode, RunRequest, ToolCall,
    ToolEffect, ToolError, ToolOutput, ToolSource, ToolSpec,
};

/// The host: every other `KitHost` method keeps its default.
#[derive(Default)]
pub(crate) struct AssistantHost {
    note: Mutex<String>,
}

fn tool(
    name: &str,
    description: &str,
    parameters: serde_json::Value,
    approval: Approval,
    effect: ToolEffect,
) -> HostTool {
    HostTool {
        spec: ToolSpec {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters,
            approval,
            effect,
            source: ToolSource::Host,
        },
    }
}

#[async_trait::async_trait]
impl KitHost for AssistantHost {
    fn tools(&self, _req: &RunRequest, _run_id: &str) -> Vec<HostTool> {
        vec![
            tool(
                "read_note",
                "Read the user's note.",
                json!({ "type": "object", "properties": {} }),
                Approval::Auto,
                ToolEffect::Read,
            ),
            tool(
                "write_note",
                "Replace the user's note with `text`.",
                json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
                Approval::Ask,
                ToolEffect::Edit,
            ),
        ]
    }

    fn permission_modes(&self, _req: &RunRequest) -> Vec<PermissionMode> {
        vec![
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
            PermissionMode::FullAccess,
        ]
    }

    async fn call_tool(
        &self,
        call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let mut note = self.note.lock().unwrap_or_else(PoisonError::into_inner);
        let text = match call.name.as_str() {
            "read_note" if note.is_empty() => "(empty note)".to_owned(),
            "read_note" => note.clone(),
            "write_note" => {
                let Some(text) = call.args.get("text").and_then(|t| t.as_str()) else {
                    return Err(ToolError {
                        code: "bad_args".to_owned(),
                        message: "`text` must be a string".to_owned(),
                        unknown: false,
                    });
                };
                text.clone_into(&mut note);
                format!("note saved ({} chars)", text.chars().count())
            }
            _ => return Err(ToolError::not_found()),
        };
        tracing::info!(tool = %call.name, "example.tool.executed");
        Ok(ToolOutput {
            text,
            payload: None,
        })
    }
}
