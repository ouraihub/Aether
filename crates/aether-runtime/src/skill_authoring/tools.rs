//! Authoring tool catalogue and dispatch; `filesystem` enforces workspace boundaries.
use super::process;
use crate::{
    Approval, CancellationToken, HostTool, ToolCall, ToolEffect, ToolError, ToolOutput, ToolSource,
    ToolSpec,
};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
#[path = "filesystem.rs"]
mod filesystem;

const MAX_FILE: usize = 1024 * 1024;
const MAX_SKILL: usize = 64 * 1024;
const MAX_OUTPUT: usize = 24 * 1024;
const MAX_VISITED: usize = 4000;

/// Filesystem-backed tools for one ephemeral authoring workspace.
///
/// `root` is never exposed to the model: all arguments are workspace-relative paths.
pub(super) struct AuthoringTools {
    root: PathBuf,
}

impl AuthoringTools {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(super) fn specs(&self) -> Vec<HostTool> {
        let schema = |properties: Value, required: &[&str]| {
            json!({
                "type": "object", "properties": properties, "required": required,
                "additionalProperties": false
            })
        };
        vec![
            host(
                "authoring_list",
                "List files in creator, skill, or evaluations.",
                schema(
                    json!({
                        "path": {"type":"string", "description":"workspace-relative directory (default root)"}
                    }),
                    &[],
                ),
                Approval::Auto,
                ToolEffect::Read,
            ),
            host(
                "authoring_read",
                "Read a UTF-8 text file from the authoring workspace.",
                schema(
                    json!({
                        "path": {"type":"string"}, "offsetChars": {"type":"integer", "minimum":0, "description":"Continue at nextOffsetChars when truncated"}
                    }),
                    &["path"],
                ),
                Approval::Auto,
                ToolEffect::Read,
            ),
            host(
                "authoring_write",
                "Atomically write a file in skill or evaluations.",
                schema(
                    json!({
                        "path": {"type":"string"}, "content": {"type":"string"}
                    }),
                    &["path", "content"],
                ),
                Approval::Auto,
                ToolEffect::Edit,
            ),
            host(
                "authoring_run",
                "Run one explicitly selected script with bounded output.",
                schema(
                    json!({
                        "path": {"type":"string"}, "args": {"type":"array", "items":{"type":"string"}},
                        "timeoutSeconds": {"type":"integer", "minimum":1, "maximum":120}
                    }),
                    &["path"],
                ),
                Approval::Ask,
                ToolEffect::Exec,
            ),
            host(
                "authoring_validate",
                "Validate and preview the pending skill package without installing it.",
                schema(json!({}), &[]),
                Approval::Auto,
                ToolEffect::Read,
            ),
        ]
    }

    pub(super) async fn execute(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(error(
                "cancelled_before_start",
                "cancelled before tool execution",
                false,
            ));
        }
        let object = call
            .args
            .as_object()
            .ok_or_else(|| error("args", "arguments must be an object", false))?;
        let keys: &[&str] = match call.name.as_str() {
            "authoring_list" => &["path"],
            "authoring_read" => &["path", "offsetChars"],
            "authoring_write" => &["path", "content"],
            "authoring_run" => &["path", "args", "timeoutSeconds"],
            "authoring_validate" => &[],
            _ => return Err(ToolError::not_found()),
        };
        if object.keys().any(|key| !keys.contains(&key.as_str())) {
            return Err(error("args", "unexpected argument", false));
        }
        if call
            .args
            .get("path")
            .is_some_and(|value| !value.is_string())
        {
            return Err(error("path", "must be a string", false));
        }
        match call.name.as_str() {
            "authoring_list" => self.list(call.args.get("path").and_then(Value::as_str)),
            "authoring_read" => self.read(
                required_str(&call.args, "path")?,
                call.args.get("offsetChars").map_or(Ok(0), |v| {
                    v.as_u64()
                        .ok_or_else(|| error("offsetChars", "must be a nonnegative integer", false))
                })? as usize,
            ),
            "authoring_write" => self.write(
                required_str(&call.args, "path")?,
                call.args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error("content", "required string", false))?,
            ),
            "authoring_run" => self.run(&call.args, cancel).await,
            "authoring_validate" => self.validate(),
            _ => Err(ToolError::not_found()),
        }
    }

    async fn run(&self, args: &Value, cancel: &CancellationToken) -> Result<ToolOutput, ToolError> {
        let raw = required_str(args, "path")?;
        let path = self.path(raw, false)?;
        let rel = path
            .strip_prefix(&self.root)
            .map_err(|_| error("path", "outside workspace", false))?;
        let mut components = rel.components();
        let Some(Component::Normal(area)) = components.next() else {
            return Err(error("path", "script path required", false));
        };
        let area = area.to_string_lossy();
        let allowed = match area.as_ref() {
            "evaluations" => true,
            "creator" => matches!(
                components.next().and_then(|c| c.as_os_str().to_str()),
                Some("scripts" | "eval-viewer")
            ),
            "skill" => components.next().and_then(|c| c.as_os_str().to_str()) == Some("scripts"),
            _ => false,
        };
        if !allowed {
            return Err(error(
                "path",
                "only creator/scripts, skill/scripts, or evaluations scripts may run",
                false,
            ));
        }
        let meta = symlink_metadata(&path)?;
        if !meta.is_file() {
            return Err(error("path", "script is not a regular file", false));
        }
        if meta.len() > MAX_FILE as u64 {
            return Err(error("path", "script exceeds size limit", false));
        }
        let interpreter = match path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "py" => "python",
            "js" | "mjs" | "cjs" => "node",
            "sh" => "sh",
            _ => return Err(error("path", "unsupported script type", false)),
        };
        let list = match args.get("args") {
            None => &[][..],
            Some(Value::Array(list)) if list.len() <= 128 => list.as_slice(),
            _ => {
                return Err(error(
                    "args",
                    "must be an array of at most 128 strings",
                    false,
                ));
            }
        };
        let mut command_args = Vec::with_capacity(list.len());
        let mut arg_bytes = 0;
        for value in list {
            let value = value
                .as_str()
                .ok_or_else(|| error("args", "must contain strings", false))?;
            arg_bytes += value.len();
            if value.contains('\0') || arg_bytes > 32 * 1024 {
                return Err(error("args", "invalid or oversized arguments", false));
            }
            command_args.push(value.to_owned());
        }
        let seconds = match args.get("timeoutSeconds") {
            None => 120,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| error("timeoutSeconds", "must be an integer", false))?,
        };
        if seconds == 0 || seconds > 120 {
            return Err(error("timeoutSeconds", "must be 1..120", false));
        }
        let output = process::run(
            &self.root,
            &path,
            interpreter,
            &command_args,
            Duration::from_secs(seconds),
            cancel,
        )
        .await
        .map_err(|reason| {
            let unknown = !matches!(
                reason.as_str(),
                "cancelled_before_start"
                    | "interpreter_not_found"
                    | "interpreter_not_executable"
                    | "spawn_failed"
            );
            error(&reason, &reason, unknown)
        })?;
        let mut text = format!(
            "exitCode: {:?}\nstdout:\n{}\nstderr:\n{}{}",
            output.exit_code,
            output.stdout,
            output.stderr,
            if output.truncated {
                "\n[truncated]"
            } else {
                ""
            }
        );
        truncate(&mut text, MAX_OUTPUT);
        Ok(ToolOutput {
            text,
            payload: Some(
                json!({"exitCode": output.exit_code, "stdout": output.stdout, "stderr": output.stderr, "truncated": output.truncated}),
            ),
        })
    }
}

fn host(
    name: &str,
    description: &str,
    parameters: Value,
    approval: Approval,
    effect: ToolEffect,
) -> HostTool {
    HostTool {
        spec: ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters,
            approval,
            effect,
            source: ToolSource::Host,
        },
    }
}
fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| error(key, "required string", false))
}
fn error(code: &str, message: &str, unknown: bool) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into(),
        unknown,
    }
}
fn io_error(e: &io::Error) -> ToolError {
    error("io_error", &e.to_string(), false)
}
fn symlink_metadata(path: &Path) -> Result<fs::Metadata, ToolError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error(&e))?;
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let linked = metadata.file_type().is_symlink();
    if linked {
        return Err(error("path", "links and junctions are forbidden", false));
    }
    Ok(metadata)
}
fn truncate(text: &mut String, max: usize) -> bool {
    if text.len() <= max {
        return false;
    }
    let mut at = max.saturating_sub(3);
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text.truncate(at);
    text.push('…');
    true
}
#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
