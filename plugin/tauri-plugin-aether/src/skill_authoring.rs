//! Window-owned skill-authoring sessions; the runtime owns workspaces,
//! runs and cleanup.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use aether_runtime::{
    Ack,
    EventSink,
    ModelRef,
    RunAccepted,
    RunContext,
    RunEvent,
    RunOwner,
    SkillAuthoringService,
};
use aether_skills::sources::PreparedSkill;
use aether_support::KitError;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, Window};

/// Window ownership for authoring sessions; a closed window never regains
/// its owner.
pub(crate) struct SkillAuthoring {
    pub(crate) service: Arc<SkillAuthoringService>,
    owners: Mutex<HashMap<String, RunOwner>>,
}

impl SkillAuthoring {
    pub(crate) fn new(service: Arc<SkillAuthoringService>) -> Self {
        Self {
            service,
            owners: Mutex::default(),
        }
    }

    pub(crate) fn register(&self, label: &str) {
        self.owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(label.to_owned())
            .or_default();
    }

    fn owner(&self, label: &str) -> Result<RunOwner, KitError> {
        self.owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(label)
            .cloned()
            .ok_or_else(|| KitError::conflict("owner_closed"))
    }

    pub(crate) fn close(&self, label: &str) {
        let owner = self
            .owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(label);
        if let Some(owner) = owner {
            self.service.cancel_owner(&owner);
        }
    }
}

fn check_id(field: &str, value: &str) -> Result<(), KitError> {
    if value.chars().count() > 128 {
        return Err(KitError::bad_request(field, "too_long").with_limit(128));
    }
    Ok(())
}

fn state<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri::State<'_, SkillAuthoring>, KitError> {
    app.try_state::<SkillAuthoring>()
        .ok_or_else(|| KitError::needs_config("skill_authoring"))
}

/// Creates an authoring workspace owned by the invoking window.
#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "skills_authoring_create"),
    err(level = "warn")
)]
pub(crate) fn skills_authoring_create<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
) -> Result<String, KitError> {
    let authoring = state(&app)?;
    authoring.service.create(&authoring.owner(window.label())?)
}

/// Sends one user turn to an owned authoring session.
#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "skills_authoring_send"),
    err(level = "warn")
)]
pub(crate) async fn skills_authoring_send<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    session_id: String,
    input: String,
    model: ModelRef,
    on_event: Channel<RunEvent>,
) -> Result<RunAccepted, KitError> {
    check_id("sessionId", &session_id)?;
    let authoring = state(&app)?;
    let owner = authoring.owner(window.label())?;
    let service = authoring.service.clone();
    let events: EventSink = Arc::new(move |event| {
        on_event.send(event).map_err(KitError::internal)
    });
    service
        .send(
            RunContext {
                owner,
                own_mcp_port: None,
            },
            &session_id,
            input,
            model,
            events,
        )
        .await
}

/// Answers a pending tool approval in an owned session.
#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "skills_authoring_approve"),
    err(level = "warn")
)]
pub(crate) fn skills_authoring_approve<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    session_id: String,
    run_id: String,
    call_id: String,
    allow: bool,
) -> Result<Ack, KitError> {
    check_id("sessionId", &session_id)?;
    check_id("runId", &run_id)?;
    check_id("callId", &call_id)?;
    let authoring = state(&app)?;
    authoring.service.approve(
        &authoring.owner(window.label())?,
        &session_id,
        &run_id,
        &call_id,
        allow,
    )
}

/// Cancels the active run of an owned session.
#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "skills_authoring_cancel"),
    err(level = "warn")
)]
pub(crate) fn skills_authoring_cancel<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    session_id: String,
    run_id: String,
) -> Result<Ack, KitError> {
    check_id("sessionId", &session_id)?;
    check_id("runId", &run_id)?;
    let authoring = state(&app)?;
    authoring
        .service
        .cancel(&authoring.owner(window.label())?, &session_id, &run_id)
}

/// Packages the session's skill directory for the normal preview + digest
/// install.
#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "skills_authoring_prepare"),
    err(level = "warn")
)]
pub(crate) fn skills_authoring_prepare<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    session_id: String,
) -> Result<PreparedSkill, KitError> {
    check_id("sessionId", &session_id)?;
    let authoring = state(&app)?;
    authoring
        .service
        .prepare(&authoring.owner(window.label())?, &session_id)
}

/// Cancels, drains and deletes an owned session workspace.
#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "skills_authoring_close"),
    err(level = "warn")
)]
pub(crate) async fn skills_authoring_close<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    session_id: String,
) -> Result<Ack, KitError> {
    check_id("sessionId", &session_id)?;
    let authoring = state(&app)?;
    let owner = authoring.owner(window.label())?;
    let service = authoring.service.clone();
    service.close(&owner, &session_id).await
}
