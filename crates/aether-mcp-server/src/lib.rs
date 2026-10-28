//! 对外 MCP 服务：回环监听、token、作用域，只暴露宿主显式注册的工具。不依赖 chat / mcp-client。
//!
//! 设计：`docs/28-详细设计-MCP服务端.md`。

mod dto;
mod handler;
mod http;
mod store;

use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub use aether_support::{KitError, KitErrorCode};
pub use dto::*;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
pub use tokio_util::sync::CancellationToken;

use crate::store::Store;

/// Default per-tool argument limit (docs/28 §5).
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 64 * 1024;

/// A boxed `Send` future, as returned by [`ToolHandler`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One tool's implementation: receives the JSON arguments (always an object).
pub type ToolHandler =
    Arc<dyn Fn(Value, CallContext) -> BoxFuture<'static, Result<Value, KitError>> + Send + Sync>;

/// A permission group the host declares; tokens and the master switch refer to its `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeSpec {
    /// Stable id, e.g. `notes.read`.
    pub id: &'static str,
    /// i18n key of the label shown in the UI.
    pub label_key: &'static str,
    /// The UI asks for a second confirmation before enabling it (docs/28 §6).
    pub destructive: bool,
}

/// A tool the host explicitly exposes to external MCP clients.
pub struct ExposedTool {
    /// Unique tool name.
    pub name: &'static str,
    /// Description shown to the external model.
    pub description: &'static str,
    /// JSON Schema of the arguments; must be an object.
    pub input_schema: Value,
    /// The declared scope a caller needs.
    pub scope: &'static str,
    /// MCP `readOnlyHint`.
    pub read_only: bool,
    /// MCP `destructiveHint`.
    pub destructive: bool,
    /// MCP `idempotentHint`.
    pub idempotent: bool,
    /// Largest accepted serialized arguments, usually [`DEFAULT_MAX_REQUEST_BYTES`].
    pub max_request_bytes: usize,
    /// The implementation.
    pub handler: ToolHandler,
}

/// Who is calling a tool.
#[derive(Debug, Clone)]
pub struct CallContext {
    /// Id of the token that authenticated the request.
    pub token_id: String,
    /// Scopes granted to that token.
    pub scopes: Vec<String>,
    /// Cancelled when the client cancels or disconnects, or the server stops.
    pub cancel: CancellationToken,
}

/// Host constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerConfig {
    /// Port used until the user picks another one (never 0, docs/13 §9.1).
    pub default_port: u16,
    /// Token prefix, e.g. `myapp` → `myapp_<base64url>`.
    pub token_prefix: &'static str,
}

struct Inner {
    store: Store,
    token_prefix: &'static str,
    scopes: Vec<ScopeSpec>,
    tools: Vec<ExposedTool>,
}

impl Inner {
    fn check_scopes(&self, ids: &[&str]) -> Result<Vec<String>, KitError> {
        let mut out: Vec<String> = Vec::with_capacity(ids.len());
        for id in ids {
            if !self.scopes.iter().any(|scope| scope.id == *id) {
                return Err(KitError::bad_request("scopes", "unknown"));
            }
            if !out.iter().any(|seen| seen == id) {
                out.push((*id).to_owned());
            }
        }
        Ok(out)
    }
}

struct Running {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

struct State {
    port: u16,
    running: Option<Running>,
}

/// The external MCP server. Create once; `start`/`stop` any number of times.
pub struct McpServer {
    inner: Arc<Inner>,
    /// Serializes `start`/`stop`/`set_port` across their awaits.
    lifecycle: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

impl McpServer {
    /// Opens `<root>/mcp-server.db` and registers the host's scopes and tools.
    ///
    /// # Errors
    /// `bad_request` for a duplicate tool name, an undeclared tool scope, a non-object
    /// input schema or a default port below 1024; database errors from opening the store.
    pub fn new(
        root: &Path,
        config: ServerConfig,
        scopes: Vec<ScopeSpec>,
        tools: Vec<ExposedTool>,
    ) -> Result<Self, KitError> {
        check_port(config.default_port)?;
        let mut names = HashSet::new();
        for tool in &tools {
            if !names.insert(tool.name) {
                return Err(KitError::bad_request("tools", "duplicate_name"));
            }
            if !scopes.iter().any(|scope| scope.id == tool.scope) {
                return Err(KitError::bad_request("tools", "undeclared_scope"));
            }
            if !tool.input_schema.is_object() {
                return Err(KitError::bad_request("tools", "schema_not_object"));
            }
        }
        let store = Store::open(root, config.default_port)?;
        let port = store.port()?;
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                token_prefix: config.token_prefix,
                scopes,
                tools,
            }),
            lifecycle: tokio::sync::Mutex::new(()),
            state: Mutex::new(State {
                port,
                running: None,
            }),
        })
    }

    /// Binds `127.0.0.1:<port>` and serves `/mcp`. Already running → current status.
    ///
    /// # Errors
    /// `conflict` with `details.port` when the port is taken.
    pub async fn start(&self) -> Result<Status, KitError> {
        let _guard = self.lifecycle.lock().await;
        self.start_locked().await
    }

    /// Stops listening and cancels in-flight calls; the port is free when this returns.
    ///
    /// # Errors
    /// Never in practice; kept fallible for symmetry with the host commands.
    pub async fn stop(&self) -> Result<Status, KitError> {
        let _guard = self.lifecycle.lock().await;
        self.stop_locked().await;
        Ok(self.status())
    }

    /// Persists a new port (1024..=65535) and restarts when running.
    ///
    /// # Errors
    /// `bad_request` for a port below 1024; `conflict` when running and the new port is
    /// taken (the server then stays stopped, the new port stays saved).
    pub async fn set_port(&self, port: u16) -> Result<Status, KitError> {
        check_port(port)?;
        let _guard = self.lifecycle.lock().await;
        self.inner.store.set_port(port)?;
        let was_running = {
            let mut state = self.lock();
            state.port = port;
            state.running.is_some()
        };
        if was_running {
            self.stop_locked().await;
            return self.start_locked().await;
        }
        Ok(self.status())
    }

    /// Issues a token; [`IssuedToken::token`] is never retrievable again.
    ///
    /// # Errors
    /// `bad_request` for an empty name, no scopes or an undeclared scope.
    pub fn issue_token(&self, name: &str, scopes: &[&str]) -> Result<IssuedToken, KitError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(KitError::bad_request("name", "empty"));
        }
        if scopes.is_empty() {
            return Err(KitError::bad_request("scopes", "empty"));
        }
        let scopes = self.inner.check_scopes(scopes)?;
        let issued = self
            .inner
            .store
            .issue(self.inner.token_prefix, name, scopes)?;
        tracing::info!(token_id = %issued.record.id, "mcp.server.token_issued");
        Ok(issued)
    }

    /// Revokes a token; the next request carrying it gets 401.
    ///
    /// # Errors
    /// `not_found` for an unknown id.
    pub fn revoke_token(&self, id: &str) -> Result<(), KitError> {
        self.inner.store.revoke(id)?;
        tracing::info!(token_id = %id, "mcp.server.token_revoked");
        Ok(())
    }

    /// All tokens, newest first, revoked ones included.
    ///
    /// # Errors
    /// Database errors.
    pub fn tokens(&self) -> Result<Vec<TokenRecord>, KitError> {
        self.inner.store.tokens()
    }

    /// Sets the user's master switch; applies to the next request.
    ///
    /// # Errors
    /// `bad_request` for an undeclared scope.
    pub fn set_enabled_scopes(&self, ids: &[&str]) -> Result<(), KitError> {
        let ids = self.inner.check_scopes(ids)?;
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.inner.store.set_enabled_scopes(&ids)
    }

    /// Declared scopes with the master-switch state.
    ///
    /// # Errors
    /// Database errors.
    pub fn scopes(&self) -> Result<Vec<ScopeState>, KitError> {
        let enabled = self.inner.store.enabled_scopes()?;
        Ok(self
            .inner
            .scopes
            .iter()
            .map(|scope| ScopeState {
                id: scope.id.to_owned(),
                label_key: scope.label_key.to_owned(),
                destructive: scope.destructive,
                enabled: enabled.iter().any(|id| id == scope.id),
            })
            .collect())
    }

    /// Current state.
    #[must_use]
    pub fn status(&self) -> Status {
        let state = self.lock();
        Status {
            running: state.running.is_some(),
            port: state.port,
            url: format!("http://127.0.0.1:{}/mcp", state.port),
            tool_count: self.inner.tools.len(),
        }
    }

    async fn start_locked(&self) -> Result<Status, KitError> {
        let port = {
            let state = self.lock();
            if state.running.is_some() {
                drop(state);
                return Ok(self.status());
            }
            state.port
        };
        // Never `bind(0)`: client configs pin the port (docs/13 §9.1).
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| {
                // Windows reports ports reserved by Hyper-V / WinNAT as access denied.
                if matches!(
                    error.kind(),
                    ErrorKind::AddrInUse | ErrorKind::PermissionDenied
                ) {
                    KitError::conflict("port_in_use").with_port(port)
                } else {
                    KitError::internal(error)
                }
            })?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(http::serve(
            listener,
            Arc::clone(&self.inner),
            cancel.clone(),
        ));
        self.lock().running = Some(Running { cancel, task });
        tracing::info!(port, tools = self.inner.tools.len(), "mcp.server.started");
        Ok(self.status())
    }

    async fn stop_locked(&self) {
        let running = self.lock().running.take();
        if let Some(running) = running {
            running.cancel.cancel();
            // The accept loop owns the listener; joining it frees the port. A panic in
            // that loop has nothing left to clean up.
            let _ = running.task.await;
            tracing::info!("mcp.server.stopped");
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // State is two plain fields with no invariant a panic could break.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Some(running) = self.lock().running.take() {
            running.cancel.cancel();
        }
    }
}

fn check_port(port: u16) -> Result<(), KitError> {
    if port < 1024 {
        return Err(KitError::bad_request("port", "out_of_range"));
    }
    Ok(())
}
