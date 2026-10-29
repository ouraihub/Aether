//! Multi-turn authoring in disposable workspaces, using the shared run engine.

mod process;
mod tools;

use crate::conversations::{ConversationStore, SqliteStore};
use crate::{
    Ack, Approval, AuthorizeContext, CancellationToken, EventSink, HostTool, KitError, KitHost,
    ModelRef, PermissionMode, RunAccepted, RunConfig, RunContext, RunDependencies, RunOwner,
    RunRequest, RunService, ToolCall, ToolError, ToolOutput,
};
use aether_providers::Store as ProviderStore;
use aether_skills::{SkillOrigin, Store as SkillStore, sources::PreparedSkill};
use aether_support::{SkillRef, net::AddressPolicy};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tempfile::TempDir;
use tools::AuthoringTools;

const PURPOSE: &str = "skill-create";
const SCOPE: &str = "aether:skill-create";
const CLOSE_GRACE: Duration = Duration::from_secs(10);
const MAX_INPUT_CHARS: usize = 12_000;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

fn creator_ref(store: &SkillStore) -> Result<SkillRef, KitError> {
    let creator = store
        .list()?
        .into_iter()
        .find(|skill| skill.name == "skill-creator" && skill.origin == SkillOrigin::Builtin)
        .ok_or_else(|| KitError::needs_config("skill_creator"))?;
    Ok(SkillRef {
        id: creator.id,
        revision: creator.revision,
    })
}

struct AuthoringHost {
    providers: Arc<ProviderStore>,
    skills: Arc<SkillStore>,
    tools: AuthoringTools,
}

#[async_trait::async_trait]
impl KitHost for AuthoringHost {
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, KitError> {
        if request.skills != vec![creator_ref(&self.skills)?] {
            return Err(KitError::needs_config("skill_creator"));
        }
        let model = crate::default_select(request)?;
        let available = self
            .providers
            .model_options(aether_providers::ModelFilter {
                model_type: Some(aether_providers::ModelType::Chat),
                ..aether_providers::ModelFilter::default()
            })?;
        if available.iter().any(|option| {
            option.endpoint_id == model.endpoint_id && option.model_id == model.model_id
        }) {
            Ok(model)
        } else {
            Err(KitError::needs_config("model"))
        }
    }

    fn system_prompt(&self, _request: &RunRequest) -> Option<String> {
        Some(concat!(
            "You are the skill authoring assistant. Follow the complete builtin skill-creator workflow, ",
            "including clarification, drafting, evaluations, comparison and iterative improvement. ",
            "The host overrides every original host path/environment assumption: creator/ contains ",
            "the builtin creator's read-only resources; skill/ is the ONLY package to install; ",
            "evaluations/ contains test scripts, inputs, outputs, benchmark reports and review artifacts ",
            "and is NEVER installed. Use authoring_list/read/write/validate and authoring_run. ",
            "Use creator/scripts and creator/agents documentation as needed; do not assume host-specific ",
            "environment variables, hidden tools, subagents, or a bundled interpreter exist. ",
            "Every process requires individual user approval. Supply explicit script path, args and ",
            "timeout, not arbitrary shell commands. Missing dependencies must be reported, not ",
            "automatically installed. These directories are a WORKSPACE, NOT an OS sandbox; approved ",
            "scripts can access the machine. Never claim an evaluation ran or passed unless actual ",
            "tool results establish that. Explain unavailable evaluation capabilities honestly. ",
            "Place SKILL.md, LICENSE and reusable scripts/references/assets under skill/ only. ",
            "Do not install the package: the user reviews its preview and confirms installation."
        ).into())
    }

    fn tools(&self, _request: &RunRequest, _run_id: &str) -> Vec<HostTool> {
        self.tools.specs()
    }
    fn permission_modes(&self, _request: &RunRequest) -> Vec<PermissionMode> {
        vec![PermissionMode::Default]
    }
    fn mcp_tools(&self, _request: &RunRequest) -> bool {
        false
    }
    async fn call_tool(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.tools.execute(call, cancel).await
    }
    fn authorize(&self, call: &ToolCall, context: &AuthorizeContext<'_>) -> Approval {
        if context.proposed == Approval::Deny {
            Approval::Deny
        } else if call.name == "authoring_run" {
            Approval::Ask
        } else {
            context.proposed
        }
    }
}

#[derive(Default)]
struct SessionState {
    closing: bool,
    // A generation also protects against End arriving before start returns.
    active: Option<String>,
    conversation_id: Option<String>,
}

struct Session {
    id: String,
    owner: RunOwner,
    run: Arc<RunService>,
    store: Arc<SqliteStore>,
    state: Mutex<SessionState>,
    root: PathBuf,
    workspace: Mutex<Option<TempDir>>,
    // Last field: SQLite handles above must close before deleting its directory.
    _database: TempDir,
}

impl Session {
    fn cleanup(&self) -> Result<(), KitError> {
        if let Some(workspace) = lock(&self.workspace).take() {
            workspace.close().map_err(KitError::internal)?;
        }
        Ok(())
    }
}

/// Shared, owner-isolated, multi-turn skill authoring with temporary persistence.
pub struct SkillAuthoringService {
    providers: Arc<ProviderStore>,
    skills: Arc<SkillStore>,
    policy: AddressPolicy,
    closing: AtomicBool,
    runtime: tokio::runtime::Handle,
    sessions: Arc<Mutex<Vec<Arc<Session>>>>,
}

impl SkillAuthoringService {
    /// Constructs the service inside an entered Tokio runtime. The captured handle
    /// also supports synchronous create/cancel_owner calls from UI event threads.
    /// # Errors
    /// Returns an internal error when no Tokio runtime is entered.
    pub fn new(
        providers: Arc<ProviderStore>,
        skills: Arc<SkillStore>,
        policy: AddressPolicy,
    ) -> Result<Arc<Self>, KitError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(KitError::internal)?;
        Ok(Arc::new(Self {
            providers,
            skills,
            policy,
            runtime,
            closing: AtomicBool::new(false),
            sessions: Arc::default(),
        }))
    }

    /// Creates a disposable workspace and a private conversation database.
    /// # Errors
    /// Rejects closed owners/services or filesystem/runtime failures.
    pub fn create(&self, owner: &RunOwner) -> Result<String, KitError> {
        if owner.closed() || self.closing.load(Ordering::Acquire) {
            return Err(KitError::cancelled());
        }
        let temp = tempfile::tempdir().map_err(KitError::internal)?;
        #[cfg(windows)]
        let root = temp.path().to_owned();
        // macOS temp paths may contain OS-owned aliases such as /var -> /private/var.
        #[cfg(not(windows))]
        let root = std::fs::canonicalize(temp.path()).map_err(KitError::internal)?;
        aether_skills::creator::write_resources(&root.join("creator"))?;
        std::fs::create_dir(root.join("skill")).map_err(KitError::internal)?;
        std::fs::create_dir(root.join("evaluations")).map_err(KitError::internal)?;
        let database = tempfile::tempdir().map_err(KitError::internal)?;
        let store = Arc::new(SqliteStore::open(database.path())?);
        let mut deps = RunDependencies::new(self.providers.clone());
        deps.skills = Some(self.skills.clone());
        deps.conversations = Some(store.clone());
        let _runtime = self.runtime.enter();
        let run = RunService::new(
            Arc::new(AuthoringHost {
                providers: self.providers.clone(),
                skills: self.skills.clone(),
                tools: AuthoringTools::new(root.clone()),
            }),
            RunConfig::default(),
            deps,
            self.policy.clone(),
        )?;
        let id = aether_support::id::new_id()?;
        let mut sessions = lock(&self.sessions);
        if owner.closed() || self.closing.load(Ordering::Acquire) {
            return Err(KitError::cancelled());
        }
        sessions.push(Arc::new(Session {
            id: id.clone(),
            owner: owner.clone(),
            run,
            store,
            state: Mutex::default(),
            root,
            workspace: Mutex::new(Some(temp)),
            _database: database,
        }));
        Ok(id)
    }

    fn session(&self, owner: &RunOwner, id: &str) -> Result<Arc<Session>, KitError> {
        lock(&self.sessions)
            .iter()
            .find(|s| s.id == id && s.owner.same_owner(owner))
            .cloned()
            .ok_or_else(|| KitError::not_found("authoring_session"))
    }

    /// Starts one authoring turn; completed turns are retained in this session only.
    /// # Errors
    /// Rejects invalid input, wrong owners, busy/closed sessions or unavailable creator/model.
    pub async fn send(
        &self,
        context: RunContext,
        session_id: &str,
        input: String,
        model: ModelRef,
        events: EventSink,
    ) -> Result<RunAccepted, KitError> {
        let session = self.session(&context.owner, session_id)?;
        if input.trim().is_empty() {
            return Err(KitError::bad_request("input", "empty"));
        }
        if input.chars().count() > MAX_INPUT_CHARS {
            return Err(
                KitError::bad_request("input", "too_long").with_limit(MAX_INPUT_CHARS as u64)
            );
        }
        let generation = aether_support::id::new_id()?;
        let (conversation_id, creator) = {
            let mut state = lock(&session.state);
            if state.closing {
                return Err(KitError::conflict("session_closed"));
            }
            if state.active.is_some() {
                return Err(KitError::conflict("session_busy"));
            }
            if context.owner.closed() || self.closing.load(Ordering::Acquire) {
                return Err(KitError::cancelled());
            }
            let creator = creator_ref(&self.skills)?;
            state.active = Some(generation.clone());
            (state.conversation_id.clone(), creator)
        };
        let sink_session = session.clone();
        let sink_generation = generation.clone();
        let sink: EventSink = Arc::new(move |event| {
            if matches!(event, crate::RunEvent::End { .. }) {
                // Save-before-End is guaranteed by RunService. Discover the reserved conversation
                // here, not via RunAccepted, which may still be in flight on a fast model.
                let summary = sink_session.store.list(Some(SCOPE), 1);
                let mut state = lock(&sink_session.state);
                if state.active.as_deref() == Some(sink_generation.as_str()) {
                    if let Ok(summary) = &summary {
                        state.conversation_id = summary.first().map(|item| item.id.clone());
                    }
                    state.active = None;
                }
                drop(state);
                summary?;
            }
            events(event)
        });
        let accepted = session
            .run
            .start(
                context,
                RunRequest {
                    purpose: PURPOSE.into(),
                    model: Some(model),
                    skills: vec![creator],
                    context: None,
                    input,
                    images: Vec::new(),
                    conversation_id,
                    scope: Some(SCOPE.into()),
                    web_search: Some(false),
                    regenerate: false,
                    reply_to_id: None,
                    retry_turn_id: None,
                    translate_turn_id: None,
                    target_language: None,
                    language: "en-US".into(),
                    reasoning_effort: None,
                    generate_image: Some(false),
                    permission_mode: Some(PermissionMode::Default),
                },
                sink,
            )
            .await;
        let mut state = lock(&session.state);
        match accepted {
            Ok(accepted) => {
                if state.closing {
                    session.run.cancel(&session.owner, &accepted.run_id)?;
                    return Err(KitError::cancelled());
                }
                if state.active.as_deref() == Some(generation.as_str()) {
                    state.conversation_id.clone_from(&accepted.conversation_id);
                }
                Ok(accepted)
            }
            Err(error) => {
                if state.active.as_deref() == Some(generation.as_str()) {
                    state.active = None;
                }
                Err(error)
            }
        }
    }

    /// Answers one pending process approval for the specified session.
    /// # Errors
    /// Rejects wrong owners, closed sessions, unknown runs and expired approvals.
    pub fn approve(
        &self,
        owner: &RunOwner,
        id: &str,
        run: &str,
        call: &str,
        allow: bool,
    ) -> Result<Ack, KitError> {
        let session = self.session(owner, id)?;
        let state = lock(&session.state);
        if state.closing {
            return Err(KitError::conflict("session_closed"));
        }
        session.run.approve(owner, run, call, allow)
    }

    /// Cancels a run within one owner-isolated session.
    /// # Errors
    /// Rejects wrong owners or closed sessions; unknown runs acknowledge without disclosure.
    pub fn cancel(&self, owner: &RunOwner, id: &str, run: &str) -> Result<Ack, KitError> {
        let session = self.session(owner, id)?;
        session.run.cancel(owner, run)
    }

    /// Prepares only skill/ through the ordinary immutable preview safety rules.
    /// # Errors
    /// Rejects active/closed sessions, unsafe files or invalid packages. Never installs.
    pub fn prepare(&self, owner: &RunOwner, id: &str) -> Result<PreparedSkill, KitError> {
        let session = self.session(owner, id)?;
        let state = lock(&session.state);
        if state.closing {
            return Err(KitError::conflict("session_closed"));
        }
        if state.active.is_some() {
            return Err(KitError::conflict("session_busy"));
        }
        let mut prepared = aether_skills::system::prepare_directory(&session.root.join("skill"))?;
        self.skills.preview(&prepared.source)?;
        prepared.origin = "Skill authoring session".into();
        Ok(prepared)
    }

    /// Permanently closes a session, cancels acceptance/execution and drains before cleanup.
    /// # Errors
    /// Rejects unknown/cross-owner sessions. Cleanup itself is bounded by the run engine.
    pub async fn close(&self, owner: &RunOwner, id: &str) -> Result<Ack, KitError> {
        let session = self.session(owner, id)?;
        lock(&session.state).closing = true;
        session.run.shutdown(CLOSE_GRACE).await;
        session.cleanup()?;
        lock(&self.sessions).retain(|s| !Arc::ptr_eq(s, &session));
        Ok(Ack { ok: true })
    }

    /// Permanently closes an owner and schedules bounded drain plus workspace cleanup.
    pub fn cancel_owner(&self, owner: &RunOwner) {
        owner.close();
        let sessions = lock(&self.sessions)
            .iter()
            .filter(|s| s.owner.same_owner(owner))
            .cloned()
            .collect::<Vec<_>>();
        for session in &sessions {
            lock(&session.state).closing = true;
            session.run.cancel_owner(owner);
        }
        let registry = self.sessions.clone();
        self.runtime.spawn(async move {
            let deadline = tokio::time::Instant::now() + CLOSE_GRACE;
            for session in sessions {
                session
                    .run
                    .shutdown(deadline.saturating_duration_since(tokio::time::Instant::now()))
                    .await;
                if let Err(error) = session.cleanup() {
                    tracing::warn!(code = ?error.code(), "skill_authoring.cleanup_failed");
                }
                lock(&registry).retain(|s| !Arc::ptr_eq(s, &session));
            }
        });
    }

    /// Stops acceptance and drains all sessions within a shared deadline.
    pub async fn shutdown(&self, grace: Duration) {
        self.closing.store(true, Ordering::Release);
        let sessions = std::mem::take(&mut *lock(&self.sessions));
        for session in &sessions {
            lock(&session.state).closing = true;
            session.run.cancel_owner(&session.owner);
        }
        let deadline = tokio::time::Instant::now() + grace;
        for session in sessions {
            session
                .run
                .shutdown(deadline.saturating_duration_since(tokio::time::Instant::now()))
                .await;
            if let Err(error) = session.cleanup() {
                tracing::warn!(code = ?error.code(), "skill_authoring.cleanup_failed");
            }
        }
    }
}

impl Drop for SkillAuthoringService {
    fn drop(&mut self) {
        for session in &*lock(&self.sessions) {
            lock(&session.state).closing = true;
            session.run.cancel_owner(&session.owner);
        }
    }
}

#[cfg(test)]
mod tests;
