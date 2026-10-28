//! Bounded script execution for skill authoring (adapted from aether-mcp-client/src/stdio.rs).

use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::ChildStderr;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const MAX_OUTPUT: usize = 10 * 1024;

#[derive(Debug)]
pub(super) struct ProcessOutput {
    pub(super) exit_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) truncated: bool,
}

fn is_secret_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.ends_with("_KEY")
        || name.ends_with("_TOKEN")
        || name.ends_with("_SECRET")
        || name.contains("PASSWORD")
}

fn same_name(left: &OsStr, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Parent environment without provider credentials; configured values are deliberately absent.
fn safe_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| !is_secret_name(&name.to_string_lossy()))
        .filter(|(name, _)| {
            same_name(name, "PATH")
                || same_name(name, "PATHEXT")
                || same_name(name, "SYSTEMROOT")
                || same_name(name, "TEMP")
                || same_name(name, "TMP")
                || same_name(name, "HOME")
        })
        .collect()
}

async fn read_limited(mut input: impl AsyncRead + Unpin) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(MAX_OUTPUT);
    let mut buf = [0_u8; 8192];
    let mut truncated = false;
    loop {
        match input.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let room = MAX_OUTPUT.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
                truncated |= n > room;
            }
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    (out, truncated)
}

/// Run one already validated script without a shell and with a killable process tree.
/// The wrapper setup intentionally mirrors the MCP stdio transport's process handling.
pub(super) async fn run(
    root: &Path,
    path: &Path,
    interpreter: &str,
    args: &[String],
    timeout_duration: Duration,
    cancel: &CancellationToken,
) -> Result<ProcessOutput, String> {
    if cancel.is_cancelled() {
        return Err("cancelled_before_start".into());
    }
    let env = safe_env();
    let mut command = CommandWrap::with_new(interpreter, |command| {
        command
            .arg(path)
            .args(args)
            .env_clear()
            .envs(env)
            .env("PYTHONPATH", root.join("creator"))
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(root);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    });
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    command.wrap(KillOnDrop);
    // io::ErrorKind is non-exhaustive; only these two kinds have a user-actionable code.
    let mut child: Box<dyn ChildWrapper> = command.spawn().map_err(|error| {
        let kind = error.kind();
        if kind == ErrorKind::NotFound {
            "interpreter_not_found".to_owned()
        } else if kind == ErrorKind::PermissionDenied {
            "interpreter_not_executable".to_owned()
        } else {
            "spawn_failed".to_owned()
        }
    })?;
    let stdout = child
        .stdout()
        .take()
        .ok_or_else(|| "stdout_unavailable".to_owned())?;
    let stderr: ChildStderr = child
        .stderr()
        .take()
        .ok_or_else(|| "stderr_unavailable".to_owned())?;
    let stdout_task = tokio::spawn(read_limited(stdout));
    let stderr_task = tokio::spawn(read_limited(stderr));
    let wait = async {
        tokio::select! {
            _ = cancel.cancelled() => Err("cancelled".to_owned()),
            result = timeout(timeout_duration, child.wait()) => result
                .map_err(|_| "timeout".to_owned())
                .and_then(|status| status.map_err(|_| "wait_failed".to_owned())),
        }
    };
    let status = match wait.await {
        Ok(status) => status,
        Err(reason) => {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(3), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(reason);
        }
    };
    // A root may exit while its descendants retain the output pipes. End the entire
    // job/group before waiting for EOF, including on an otherwise successful exit.
    let _ = child.start_kill();
    let (stdout_result, stderr_result) =
        tokio::join!(finish_output(stdout_task), finish_output(stderr_task));
    let (stdout, stdout_truncated) = stdout_result?;
    let (stderr, stderr_truncated) = stderr_result?;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let secrets: Vec<String> = std::env::vars()
        .filter_map(|(name, value)| is_secret_name(&name).then_some(value))
        .collect();
    let known: Vec<&str> = secrets.iter().map(String::as_str).collect();
    Ok(ProcessOutput {
        exit_code: status.code(),
        stdout: aether_support::mask::redact_secrets(&aether_support::mask::redact(
            &stdout, &known,
        )),
        stderr: aether_support::mask::redact_secrets(&aether_support::mask::redact(
            &stderr, &known,
        )),
        truncated: stdout_truncated || stderr_truncated,
    })
}

async fn finish_output(
    mut task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
) -> Result<(Vec<u8>, bool), String> {
    match timeout(Duration::from_secs(2), &mut task).await {
        Ok(result) => result.map_err(|_| "output_failed".to_owned()),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("output_drain_timeout".to_owned())
        }
    }
}
