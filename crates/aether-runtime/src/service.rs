//! Run acceptance, ownership, cancellation and save-before-End finalization.

use crate::conversations::{ConversationStore, TurnHandle, TurnRecord};
use crate::tools::{Approvals, HostTools};
use crate::{Ack, CancellationToken, KitHost, RunAccepted, RunRequest, RunSnapshot, RunStatus};
use aether_chat::{Budget, EndStatus, RunEngine, RunEvent, RunInput, RunSummary, Usage};
use aether_support::net::AddressPolicy;
use aether_support::{KitError, KitErrorCode};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Duration;
use tokio::task::JoinHandle;

/// Defaults and approval timeout for Native runs.
#[derive(Debug, Clone, Copy)]
pub struct RunConfig {
    /// Model limits override known output/context values.
    pub budget: Budget,
    /// Maximum time a pending approval waits.
    pub approval_timeout: Duration,
}
impl Default for RunConfig {
    fn default() -> Self {
        Self {
            budget: Budget::default(),
            approval_timeout: Duration::from_secs(300),
        }
    }
}

/// Shared capability instances pinned during preparation.
#[derive(Clone)]
pub struct RunDependencies {
    /// Model configuration and encrypted credentials.
    pub providers: Arc<aether_providers::Store>,
    /// Optional persistence, never replaced with a fake temporary store.
    pub conversations: Option<Arc<dyn ConversationStore>>,
    /// Optional pinned skill store.
    #[cfg(feature = "skills")]
    pub skills: Option<Arc<aether_skills::Store>>,
    /// Optional search configuration store.
    #[cfg(feature = "search")]
    pub search: Option<Arc<aether_search::Store>>,
    /// Optional MCP runtime shared with configuration adapters.
    #[cfg(feature = "mcp-client")]
    pub mcp: Option<Arc<aether_mcp_client::McpRuntime>>,
}
impl RunDependencies {
    /// Starts with no optional capabilities, independent of Cargo feature unification.
    #[must_use]
    pub fn new(providers: Arc<aether_providers::Store>) -> Self {
        Self {
            providers,
            conversations: None,
            #[cfg(feature = "skills")]
            skills: None,
            #[cfg(feature = "search")]
            search: None,
            #[cfg(feature = "mcp-client")]
            mcp: None,
        }
    }
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
struct OwnerState {
    id: u64,
    closed: AtomicBool,
}
/// Opaque process-local run identity. Clones share permanent closure state.
#[derive(Clone)]
pub struct RunOwner(Arc<OwnerState>);
impl RunOwner {
    /// Creates a non-reused process-local identity; identifier exhaustion is fail-stop.
    #[must_use]
    pub fn new() -> Self {
        let id = NEXT_OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .unwrap_or_else(|_| std::process::abort());
        Self(Arc::new(OwnerState {
            id,
            closed: AtomicBool::new(false),
        }))
    }
    pub(crate) fn closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }
    #[cfg(feature = "skills")]
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
    #[cfg(feature = "skills")]
    pub(crate) fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
    }
}
impl Default for RunOwner {
    fn default() -> Self {
        Self::new()
    }
}
/// Trusted adapter context; never deserialized from a request body.
pub struct RunContext {
    /// Owning application/window identity.
    pub owner: RunOwner,
    /// Local MCP server port to exclude from discovery.
    pub own_mcp_port: Option<u16>,
}
/// Fast synchronous event delivery. Failure requests cancellation.
pub type EventSink = Arc<dyn Fn(RunEvent) -> Result<(), KitError> + Send + Sync>;
struct Record {
    owner: RunOwner,
    snapshot: RunSnapshot,
    cancel: Option<CancellationToken>,
}
#[derive(Default)]
struct Registry {
    records: HashMap<String, Record>,
    completed: VecDeque<String>,
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Complete UI-independent Native run use case.
pub struct RunService {
    host: Arc<dyn KitHost>,
    config: RunConfig,
    deps: RunDependencies,
    policy: AddressPolicy,
    engine: Arc<RunEngine>,
    registry: Mutex<Registry>,
    approvals: Arc<Approvals>,
    closing: Arc<AtomicBool>,
    preparing: AtomicUsize,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    drain: tokio::sync::Mutex<()>,
    maintenance_cancel: CancellationToken,
    maintenance: Mutex<Option<JoinHandle<()>>>,
}
struct HostRun {
    host: Arc<dyn KitHost>,
    run_id: String,
}
impl Drop for HostRun {
    fn drop(&mut self) {
        self.host.run_finished(&self.run_id);
    }
}
struct Preparing<'a>(&'a AtomicUsize);
impl Drop for Preparing<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl RunService {
    /// Constructs the service inside an entered Tokio runtime when persistence is enabled.
    ///
    /// # Errors
    /// Invalid network/runtime configuration fails before any run is accepted.
    pub fn new(
        host: Arc<dyn KitHost>,
        config: RunConfig,
        deps: RunDependencies,
        policy: AddressPolicy,
    ) -> Result<Arc<Self>, KitError> {
        let engine = Arc::new(RunEngine::new(policy.clone())?);
        let cancel = CancellationToken::new();
        let maintenance = if let Some(store) = deps.conversations.clone() {
            let handle = tokio::runtime::Handle::try_current().map_err(KitError::internal)?;
            let token = cancel.clone();
            Some(handle.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(3600));
                loop {
                    tokio::select! { biased; () = token.cancelled() => break, _ = interval.tick() => {} }
                    let store = Arc::clone(&store);
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX));
                    let result = tokio::task::spawn_blocking(move || store.purge_images(now.saturating_sub(crate::conversations::IMAGE_TTL_DAYS * 86_400_000))).await;
                    match result { Ok(Ok(_)) => {}, Ok(Err(error)) => tracing::warn!(code = ?error.code(), "runtime.images.purge_failed"), Err(_) => tracing::warn!("runtime.images.purge_panicked") }
                }
            }))
        } else {
            None
        };
        Ok(Arc::new(Self {
            host,
            config,
            deps,
            policy,
            engine,
            registry: Mutex::default(),
            approvals: Arc::new(Approvals::new(config.approval_timeout)),
            closing: Arc::new(AtomicBool::new(false)),
            preparing: AtomicUsize::new(0),
            tasks: Mutex::default(),
            drain: tokio::sync::Mutex::new(()),
            maintenance_cancel: cancel,
            maintenance: Mutex::new(maintenance),
        }))
    }

    fn open(&self, owner: &RunOwner) -> Result<(), KitError> {
        if self.closing.load(Ordering::Acquire) || owner.closed() {
            Err(KitError::cancelled())
        } else {
            Ok(())
        }
    }

    /// Prepares, transactionally reserves and starts a run. Accepted failures use End only.
    ///
    /// # Errors
    /// Rejected requests leave no candidate and emit no events.
    pub async fn start(
        self: &Arc<Self>,
        context: RunContext,
        request: RunRequest,
        events: EventSink,
    ) -> Result<RunAccepted, KitError> {
        let service = self.clone();
        tokio::spawn(async move { service.accept(context, request, events).await })
            .await
            .map_err(KitError::internal)?
    }

    async fn accept(
        self: &Arc<Self>,
        context: RunContext,
        request: RunRequest,
        events: EventSink,
    ) -> Result<RunAccepted, KitError> {
        self.preparing.fetch_add(1, Ordering::AcqRel);
        let _preparing = Preparing(&self.preparing);
        self.open(&context.owner)?;
        crate::prepare::check_request(&request, self.deps.conversations.is_some())?;
        let run_id = aether_support::id::new_id()?;
        // The guard exists before the first host call, including failed preparations.
        let guard = HostRun {
            host: self.host.clone(),
            run_id: run_id.clone(),
        };
        let (host, config, deps, policy, task_id) = (
            self.host.clone(),
            self.config,
            self.deps.clone(),
            self.policy.clone(),
            run_id.clone(),
        );
        let (mut prepared, guard) = tokio::task::spawn_blocking(move || {
            let prepared = crate::prepare::prepare(host, config, &deps, policy, request, task_id)?;
            Ok::<_, KitError>((prepared, guard))
        })
        .await
        .map_err(KitError::internal)??;
        self.open(&context.owner)?;
        let cancel = self.maintenance_cancel.child_token();
        #[cfg(feature = "mcp-client")]
        if prepared.mcp_tools
            && let Some(runtime) = &self.deps.mcp
        {
            let session = runtime.session(&cancel, context.own_mcp_port).await;
            prepared
                .input
                .tools
                .extend(
                    session
                        .tools()
                        .into_iter()
                        .map(|tool| aether_chat::ToolSpec {
                            name: tool.prefixed,
                            description: tool.description,
                            parameters: tool.schema,
                            approval: tool.approval,
                            effect: crate::ToolEffect::Other,
                            source: crate::ToolSource::Mcp {
                                server: tool.server,
                            },
                        }),
                );
            prepared.tools.mcp = Some(tokio::sync::Mutex::new(session));
        }
        // Final pure validation is repeated with transaction-authoritative history below.
        self.engine.validate(&mut prepared.input)?;
        self.open(&context.owner)?;
        let (input, mut tools, request, turn, search) = {
            let crate::prepare::Prepared {
                input,
                tools,
                request,
                action,
                #[cfg(feature = "search")]
                search,
                ..
            } = prepared;
            let (input, turn) = if let Some(store) = self.deps.conversations.clone() {
                let (closing, owner, engine) = (
                    self.closing.clone(),
                    context.owner.clone(),
                    self.engine.clone(),
                );
                tokio::task::spawn_blocking(move || {
                    let mut input = Some(input);
                    let reserved = store.reserve_action(&action, &mut |source| {
                        let mut value = input
                            .take()
                            .ok_or_else(|| KitError::conflict("validation_repeated"))?;
                        crate::prepare::apply_history(&mut value, source);
                        engine.validate(&mut value)?;
                        if closing.load(Ordering::Acquire) || owner.closed() {
                            return Err(KitError::cancelled());
                        }
                        Ok(value)
                    })?;
                    Ok::<_, KitError>((reserved.run_input, Some(reserved.handle)))
                })
                .await
                .map_err(KitError::internal)??
            } else {
                (input, None)
            };
            #[cfg(not(feature = "search"))]
            let search = ();
            (input, tools, request, turn, search)
        };
        let accepted = RunAccepted {
            run_id: run_id.clone(),
            conversation_id: turn.as_ref().map(|turn| turn.conversation_id.clone()),
            turn_id: turn.as_ref().map(|turn| turn.turn_id.clone()),
        };
        if let (Some(painting), Some(store), Some(turn)) = (
            &mut tools.painting,
            self.deps.conversations.clone(),
            turn.as_ref(),
        ) {
            painting.set_turn(store, turn.turn_id.clone());
        }
        {
            let mut registry = lock(&self.registry);
            let closed = self.closing.load(Ordering::Acquire) || context.owner.closed();
            if closed && turn.is_none() {
                return Err(KitError::cancelled());
            }
            if closed {
                cancel.cancel();
            }
            registry.records.insert(
                run_id.clone(),
                Record {
                    owner: context.owner,
                    snapshot: RunSnapshot {
                        run_id: run_id.clone(),
                        conversation_id: accepted.conversation_id.clone(),
                        turn_id: accepted.turn_id.clone(),
                        status: RunStatus::Running,
                        cancel_requested: closed,
                        error: None,
                        exhausted: None,
                    },
                    cancel: Some(cancel.clone()),
                },
            );
        }
        let service = self.clone();
        let task = tokio::spawn(async move {
            let _guard = guard;
            service
                .execute(execution::Execution {
                    run_id,
                    input,
                    tools,
                    request,
                    turn,
                    cancel,
                    events,
                    search,
                })
                .await;
        });
        let mut tasks = lock(&self.tasks);
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
        Ok(accepted)
    }

    /// Idempotently requests cancellation without exposing another owner's runs.
    ///
    /// # Errors
    /// Never rejects based on run existence or ownership; always acknowledges.
    pub fn cancel(&self, owner: &RunOwner, run_id: &str) -> Result<Ack, KitError> {
        let mut registry = lock(&self.registry);
        if let Some(record) = registry
            .records
            .get_mut(run_id)
            .filter(|record| record.owner.0.id == owner.0.id)
            && let Some(cancel) = &record.cancel
        {
            record.snapshot.cancel_requested = true;
            cancel.cancel();
            self.approvals.remove_run(run_id);
        }
        Ok(Ack { ok: true })
    }
    /// Consumes a currently pending approval for this owner.
    ///
    /// # Errors
    /// Unknown/cross-owner runs return not_found; settled or cancelled approvals conflict.
    pub fn approve(
        &self,
        owner: &RunOwner,
        run_id: &str,
        call_id: &str,
        allow: bool,
    ) -> Result<Ack, KitError> {
        let registry = lock(&self.registry);
        let record = registry
            .records
            .get(run_id)
            .filter(|record| record.owner.0.id == owner.0.id)
            .ok_or_else(|| KitError::not_found("run"))?;
        if record
            .cancel
            .as_ref()
            .is_none_or(CancellationToken::is_cancelled)
            || !self.approvals.answer(run_id, call_id, allow)
        {
            return Err(KitError::conflict("approval_expired"));
        }
        Ok(Ack { ok: true })
    }
    /// Returns bounded status only, never accumulated output or tool arguments.
    ///
    /// # Errors
    /// Unknown, evicted or cross-owner runs return not_found.
    pub fn snapshot(&self, owner: &RunOwner, run_id: &str) -> Result<RunSnapshot, KitError> {
        lock(&self.registry)
            .records
            .get(run_id)
            .filter(|record| record.owner.0.id == owner.0.id)
            .map(|record| {
                let mut snapshot = record.snapshot.clone();
                snapshot.cancel_requested |= record
                    .cancel
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled);
                snapshot
            })
            .ok_or_else(|| KitError::not_found("run"))
    }
    /// Permanently closes the shared owner before scanning the short registry lock.
    pub fn cancel_owner(&self, owner: &RunOwner) {
        owner.0.closed.store(true, Ordering::Release);
        for (id, record) in &mut lock(&self.registry).records {
            if record.owner.0.id == owner.0.id
                && let Some(cancel) = &record.cancel
            {
                record.snapshot.cancel_requested = true;
                cancel.cancel();
                self.approvals.remove_run(id);
            }
        }
    }
#[path = "service_shutdown.rs"]
mod shutdown;
#[path = "service_execution.rs"]
mod execution;
