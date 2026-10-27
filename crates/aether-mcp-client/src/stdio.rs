//! stdio transport (docs/13 §3): a local process per session speaking newline-delimited
//! JSON-RPC. The process starts on demand and its whole tree ends with the link.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::Stdio;

use aether_support::error::upstream_excerpt;
use aether_support::{KitError, KitErrorCode};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::protocol::{MAX_MESSAGE_BYTES, RpcError, Transport, find, too_large};

/// stderr kept for the log summary; the rest is read and discarded.
const MAX_STDERR_BYTES: usize = 64 * 1024;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What to start; validated when saved (docs/13 §4).
pub(crate) struct Launch<'a> {
    /// Server id, for logs only.
    pub(crate) server_id: &'a str,
    pub(crate) command: &'a str,
    pub(crate) args: &'a [String],
    pub(crate) cwd: Option<&'a str>,
    pub(crate) env: &'a BTreeMap<String, String>,
}

/// One running server process.
pub(crate) struct StdioLink {
    /// `None` once terminated.
    child: Option<Child>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// The line being read; kept when a read is dropped mid-way.
    line: Vec<u8>,
    known: Vec<String>,
    alive: bool,
}

impl StdioLink {
    /// Starts the process with the filtered parent environment plus the configured one.
    pub(crate) fn spawn(launch: &Launch<'_>, known: Vec<String>) -> Result<Self, KitError> {
        let env = child_env(std::env::vars_os(), launch.env);
        let program = resolve_program(launch.command, &env);
        let mut command = Command::new(program);
        command
            .args(launch.args)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = launch.cwd {
            command.current_dir(cwd);
        }
        #[cfg(windows)]
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| {
            let kind = error.kind();
            if kind == ErrorKind::NotFound {
                KitError::bad_request("command", "not_found").with_cause(error)
            } else if kind == ErrorKind::PermissionDenied {
                KitError::bad_request("command", "not_executable").with_cause(error)
            } else {
                KitError::new(KitErrorCode::Upstream, "MCP server could not be started")
                    .with_reason("spawn_failed")
                    .with_cause(error)
            }
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(KitError::new(KitErrorCode::Internal, "child pipes missing"));
        };
        tokio::spawn(drain_stderr(
            stderr,
            launch.server_id.to_owned(),
            known.clone(),
        ));
        tracing::debug!(server_id = %launch.server_id, "mcp.client.stdio_started");
        Ok(Self {
            child: Some(child),
            stdin,
            stdout: BufReader::new(stdout),
            line: Vec::new(),
            known,
            alive: true,
        })
    }

    async fn send(&mut self, message: &Value) -> Result<(), KitError> {
        let mut line = serde_json::to_vec(message).map_err(KitError::internal)?;
        line.push(b'\n');
        let written = async {
            self.stdin.write_all(&line).await?;
            self.stdin.flush().await
        }
        .await;
        written.map_err(|error| {
            self.alive = false;
            exited().with_cause(error)
        })
    }

    /// One line without its terminator, at most `MAX_MESSAGE_BYTES`.
    async fn read_line(&mut self) -> Result<Vec<u8>, KitError> {
        loop {
            let available = match self.stdout.fill_buf().await {
                Ok(available) => available,
                Err(error) => {
                    self.alive = false;
                    return Err(exited().with_cause(error));
                }
            };
            if available.is_empty() {
                self.alive = false;
                return Err(exited());
            }
            let newline = available.iter().position(|&b| b == b'\n');
            let take = newline.map_or(available.len(), |at| at + 1);
            if self.line.len() + newline.unwrap_or(take) > MAX_MESSAGE_BYTES {
                self.alive = false;
                return Err(too_large());
            }
            self.line.extend_from_slice(&available[..take]);
            self.stdout.consume(take);
            if newline.is_some() {
                let mut line = std::mem::take(&mut self.line);
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
        }
    }

    /// Server-to-client requests: `ping` is answered, anything else is "method not found".
    async fn answer(&mut self, request: &Value) -> Result<(), KitError> {
        let id = &request["id"];
        let reply = if request["method"] == "ping" {
            json!({ "jsonrpc": "2.0", "id": id, "result": {} })
        } else {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } })
        };
        self.send(&reply).await
    }

    /// Ends the process tree. The tree goes first: once the root is gone its children can
    /// no longer be found. Runs on the runtime so dropping never blocks.
    fn terminate(&mut self) {
        self.alive = false;
        let Some(mut child) = self.child.take() else {
            return;
        };
        // An exited root was reaped by `try_wait`; its pid may already belong to another
        // process, so only a running root is used to find the tree.
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        let Some(pid) = child.id() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = kill_tree(pid).status().await {
                    tracing::debug!(error = %error, "mcp.client.stdio_kill_tree_failed");
                }
                // Already gone after a successful tree kill; this also reaps it.
                let _ = child.kill().await;
            });
        }
        // Without a runtime `child` is dropped here and `kill_on_drop` ends the root.
    }
}

impl Transport for StdioLink {
    async fn request(&mut self, message: &Value, id: u64) -> Result<Value, RpcError> {
        self.send(message).await.map_err(RpcError::Failed)?;
        loop {
            let line = self.read_line().await.map_err(RpcError::Unknown)?;
            let Ok(reply) = serde_json::from_slice::<Value>(&line) else {
                tracing::debug!(bytes = line.len(), "mcp.client.stdio_non_json_line");
                continue;
            };
            if reply.get("method").is_some() {
                if reply.get("id").is_some() {
                    self.answer(&reply).await.map_err(RpcError::Unknown)?;
                }
                continue;
            }
            if let Some(reply) = find(reply, id) {
                return Ok(reply);
            }
        }
    }

    async fn notify(&mut self, message: &Value) -> Result<(), RpcError> {
        self.send(message).await.map_err(RpcError::Failed)
    }

    /// stdio has no server-side session.
    fn reset(&mut self) {}

    /// A request left half-written or half-read cannot be resynchronised: end the process.
    fn abandon(&mut self) {
        self.terminate();
    }

    fn is_alive(&self) -> bool {
        self.alive
    }

    fn known(&self) -> &[String] {
        &self.known
    }
}

impl Drop for StdioLink {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn exited() -> KitError {
    KitError::new(KitErrorCode::Network, "MCP server exited").with_reason("server_exited")
}

/// The parent environment without credential-looking names, then the configured variables.
pub(crate) fn child_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    configured: &BTreeMap<String, String>,
) -> Vec<(OsString, OsString)> {
    parent
        .into_iter()
        .filter(|(name, _)| !is_secret_name(&name.to_string_lossy()))
        .chain(
            configured
                .iter()
                .map(|(name, value)| (name.into(), value.into())),
        )
        .collect()
}

/// `*_KEY`, `*_TOKEN`, `*_SECRET`, `*PASSWORD*`, case-insensitive like Windows names.
fn is_secret_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.ends_with("_KEY")
        || name.ends_with("_TOKEN")
        || name.ends_with("_SECRET")
        || name.contains("PASSWORD")
}

/// Windows `CreateProcess` only appends `.exe` to a bare name, so the usual MCP launchers
/// (`npx.cmd`, `uvx.cmd`) are "not found". A bare name is looked up in the child's `PATH`
/// with its `PATHEXT`, like a shell; anything with a separator or extension is used as is.
/// std quotes `.cmd`/`.bat` arguments safely (CVE-2024-24576 fix), so no shell is added.
#[cfg(windows)]
pub(crate) fn resolve_program(command: &str, env: &[(OsString, OsString)]) -> OsString {
    let path = std::path::Path::new(command);
    if path.components().count() != 1 || path.extension().is_some() {
        return command.into();
    }
    let var = |key: &str| {
        env.iter()
            .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case(key))
            .map(|(_, value)| value.clone())
    };
    let exts = var("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let exts: Vec<String> = exts
        .to_string_lossy()
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(str::to_owned)
        .collect();
    let dirs = var("PATH").unwrap_or_default();
    std::env::split_paths(&dirs)
        .flat_map(|dir| {
            exts.iter()
                .map(move |ext| dir.join(format!("{command}{ext}")))
        })
        .find(|candidate| candidate.is_file())
        .map_or_else(|| command.into(), Into::into)
}

#[cfg(not(windows))]
pub(crate) fn resolve_program(command: &str, _env: &[(OsString, OsString)]) -> OsString {
    command.into()
}

#[cfg(windows)]
fn kill_tree(pid: u32) -> Command {
    let mut command = Command::new("taskkill");
    command
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW);
    quiet(command)
}

#[cfg(not(windows))]
fn kill_tree(pid: u32) -> Command {
    // The child leads its own process group (`process_group(0)`), so `-pid` is the tree.
    let mut command = Command::new("kill");
    command.args(["-s", "KILL", "--", &format!("-{pid}")]);
    quiet(command)
}

fn quiet(mut command: Command) -> Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Reads stderr to the end so the process never blocks on it; logs a redacted excerpt of
/// the first 64 KiB. stderr never reaches error messages.
async fn drain_stderr(mut stderr: impl AsyncRead + Unpin, server_id: String, known: Vec<String>) {
    let mut kept = Vec::new();
    let mut total = 0_usize;
    let mut chunk = [0_u8; 4096];
    while let Ok(n) = stderr.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        total += n;
        let room = MAX_STDERR_BYTES.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..n.min(room)]);
    }
    if total > 0 {
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        let excerpt = upstream_excerpt(&String::from_utf8_lossy(&kept), &known);
        tracing::info!(server_id = %server_id, bytes = total, excerpt = %excerpt, "mcp.client.stdio_stderr");
    }
}
