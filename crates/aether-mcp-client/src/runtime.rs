//! Connections, fixed per-run catalogues and owned cache refresh tasks (docs/27).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_support::{KitError, secret};
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::credentials::Secret;
use crate::http::HttpLink;
use crate::protocol::{Connection, Link, REQUEST_TIMEOUT};
use crate::session::{McpSession, SessionServer};
use crate::stdio::{Launch, StdioLink};
use crate::store::{Target, check_stdio, now_ms};
use crate::{McpTransport, Store, ToolDto};

const REFRESH_AFTER_MS: i64 = 10 * 60 * 1000;

#[derive(Clone, Copy)]
pub(crate) struct Timeouts {
    pub(crate) request: Duration,
    pub(crate) startup: Duration,
}

/// Owns background discovery and the irreversible client shutdown boundary.
/// Constructing this value performs no I/O and starts no tasks.
pub struct McpRuntime {
    store: Arc<Store>,
    pub(crate) timeouts: Timeouts,
    shutdown: CancellationToken,
    tasks: Mutex<JoinSet<()>>,
    drain: tokio::sync::Mutex<()>,
}

impl McpRuntime {
    /// Creates a runtime over the shared synchronous configuration repository.
    #[must_use]
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            timeouts: Timeouts {
                request: REQUEST_TIMEOUT,
                startup: Duration::from_secs(10),
            },
            shutdown: CancellationToken::new(),
            tasks: Mutex::new(JoinSet::new()),
            drain: tokio::sync::Mutex::new(()),
        }
    }

    /// Borrows the same configuration repository used by this runtime.
    /// Settings commands remain synchronous Store operations, not runtime proxies.
    #[must_use]
    pub const fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Explicitly tests a configured connection and replaces its successful cache.
    /// Known tool approvals are retained; failed discovery leaves the cache unchanged.
    ///
    /// # Errors
    /// Returns configuration, credential, transport, protocol or cancellation errors.
    pub async fn refresh(
        &self,
        server_id: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<ToolDto>, KitError> {
        check_cancel(cancel, &self.shutdown)?;
        let target = self.store.target(server_id)?;
        refresh_target(&self.store, &target, self.timeouts, cancel, &self.shutdown)
            .await
            .map(|(_, tools)| tools)
    }

    /// Prepares a fixed directory of enabled, trusted servers for one run.
    /// Missing caches are discovered now; stale caches are used immediately and
    /// refreshed by an owned task independent of this run's cancellation.
    pub async fn session(&self, cancel: &CancellationToken, own_port: Option<u16>) -> McpSession {
        let mut servers = Vec::new();
        if check_cancel(cancel, &self.shutdown).is_ok() {
            match self.store.active_targets() {
                Ok(targets) => {
                    for target in targets {
                        if check_cancel(cancel, &self.shutdown).is_err() {
                            break;
                        }
                        if own_port.is_some_and(|port| is_local_port(&target, port)) {
                            tracing::warn!(server_id = %target.id, "mcp.client.self_server_skipped");
                            continue;
                        }
                        match self.session_server(&target, cancel).await {
                            Ok((connection, tools)) => servers.push(SessionServer {
                                target,
                                connection,
                                tools,
                            }),
                            Err(error) => {
                                tracing::warn!(server_id = %target.id, code = ?error.code(), "mcp.client.server_unavailable");
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(code = ?error.code(), "mcp.client.session_failed");
                }
            }
        }
        if check_cancel(cancel, &self.shutdown).is_err() {
            servers.clear();
        }
        McpSession::new(servers, self.timeouts, self.shutdown.clone())
    }

    async fn session_server(
        &self,
        target: &Target,
        cancel: &CancellationToken,
    ) -> Result<(Option<Connection>, Vec<crate::McpTool>), KitError> {
        let (connection, tools) = match self.store.cached_tools(&target.id)? {
            Some((fetched_at, tools)) => {
                if now_ms().saturating_sub(fetched_at) > REFRESH_AFTER_MS {
                    self.refresh_in_background(&target.id);
                }
                (None, tools)
            }
            None => {
                let (connection, _) =
                    refresh_target(&self.store, target, self.timeouts, cancel, &self.shutdown)
                        .await?;
                let tools = self
                    .store
                    .cached_tools(&target.id)?
                    .map(|(_, tools)| tools)
                    .unwrap_or_default();
                (Some(connection), tools)
            }
        };
        Ok((connection, tools))
    }

    fn refresh_in_background(&self, server_id: &str) {
        // The same lock orders admission against shutdown; no task can be registered
        // after shutdown cancels the token and starts draining the set.
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            observe_task(result);
        }
        if self.shutdown.is_cancelled() {
            return;
        }
        let server_id = server_id.to_owned();
        let store = Arc::clone(&self.store);
        let timeouts = self.timeouts;
        let cancel = self.shutdown.child_token();
        tasks.spawn(async move {
            let result = match store.target(&server_id) {
                Ok(target) => refresh_target(&store, &target, timeouts, &cancel, &cancel)
                    .await.map(drop),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                tracing::warn!(server_id = %server_id, code = ?error.code(), "mcp.client.refresh_failed");
            }
        });
    }

    /// Stops admission, cancels in-flight work and awaits every managed refresh.
    /// Concurrent callers all wait until the same drain has finished. Irreversible.
    pub async fn shutdown(&self) {
        {
            let _tasks = self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.shutdown.cancel();
        }
        let _drain = self.drain.lock().await;
        // Keep handles in their owner even if this shutdown future is cancelled.
        // The synchronous guard exists only inside a poll, never across an await.
        while let Some(result) = std::future::poll_fn(|cx| {
            self.tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .poll_join_next(cx)
        })
        .await
        {
            observe_task(result);
        }
    }
}

impl Drop for McpRuntime {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.tasks
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .abort_all();
    }
}

fn observe_task(result: Result<(), JoinError>) {
    if let Err(error) = result {
        tracing::warn!(
            cancelled = error.is_cancelled(),
            "mcp.client.refresh_task_failed"
        );
    }
}

pub(crate) fn check_cancel(
    cancel: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<(), KitError> {
    if cancel.is_cancelled() || shutdown.is_cancelled() {
        Err(KitError::cancelled())
    } else {
        Ok(())
    }
}

fn is_local_port(target: &Target, port: u16) -> bool {
    let McpTransport::Http { url } = &target.transport else {
        return false;
    };
    Url::parse(url).is_ok_and(|url| {
        url.port_or_known_default() == Some(port)
            && match url.host() {
                Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            }
    })
}

pub(crate) async fn connect(
    target: &Target,
    timeouts: Timeouts,
    cancel: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<Connection, KitError> {
    check_cancel(cancel, shutdown)?;
    let secret = match &target.sealed {
        Some(sealed) => {
            let plain = secret::open(sealed)?;
            serde_json::from_str(&plain).map_err(|_| KitError::needs_config("key_unreadable"))?
        }
        None => Secret::default(),
    };
    check_cancel(cancel, shutdown)?;
    match &target.transport {
        McpTransport::Http { url } => {
            let link = HttpLink::new(url, &secret, target.allow_private)?;
            Connection::open(
                Link::Http(Box::new(link)),
                timeouts.request,
                timeouts.request,
                cancel,
                shutdown,
            )
            .await
        }
        McpTransport::Stdio { command, args, cwd } => {
            check_stdio(command, args, cwd.as_deref())?;
            check_cancel(cancel, shutdown)?;
            let launch = Launch {
                server_id: &target.id,
                command,
                args,
                cwd: cwd.as_deref(),
                env: &secret.env,
            };
            let link = StdioLink::spawn(&launch, secret.values())?;
            Connection::open(
                Link::Stdio(Box::new(link)),
                timeouts.request,
                timeouts.startup,
                cancel,
                shutdown,
            )
            .await
        }
    }
}

async fn refresh_target(
    store: &Store,
    target: &Target,
    timeouts: Timeouts,
    cancel: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<(Connection, Vec<ToolDto>), KitError> {
    let mut connection = connect(target, timeouts, cancel, shutdown).await?;
    let tools = connection.list_tools(cancel, shutdown).await?;
    check_cancel(cancel, shutdown)?;
    let tools = store.replace_tools(&target.id, tools)?;
    Ok((connection, tools))
}
