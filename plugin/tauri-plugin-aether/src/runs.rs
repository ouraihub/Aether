//! Trusted Tauri window identity and the four runtime run commands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use aether_runtime::{
    Ack, RunAccepted, RunContext, RunEvent, RunOwner, RunRequest, RunService, RunSnapshot,
};
use aether_support::KitError;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, Window};

pub(crate) struct Runs {
    pub(crate) service: Arc<RunService>,
    owners: Mutex<HashMap<String, RunOwner>>,
}

impl Runs {
    pub(crate) fn new(service: Arc<RunService>) -> Self {
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
        // Serialize lookup/removal with registration; commands never create an identity.
        let mut owners = self.owners.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(owner) = owners.remove(label) {
            self.service.cancel_owner(&owner);
        }
    }
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "chat_run"), err(level = "warn"))]
pub(crate) async fn chat_run<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    request: RunRequest,
    on_event: Channel<RunEvent>,
) -> Result<RunAccepted, KitError> {
    let runs = app.state::<Runs>();
    let owner = runs.owner(window.label())?;
    #[cfg(feature = "mcp-server")]
    let own_mcp_port = {
        let status = app.state::<aether_mcp_server::McpServer>().status();
        status.running.then_some(status.port)
    };
    #[cfg(not(feature = "mcp-server"))]
    let own_mcp_port = None;
    runs.service
        .start(
            RunContext {
                owner,
                own_mcp_port,
            },
            request,
            Arc::new(move |event| on_event.send(event).map_err(KitError::internal)),
        )
        .await
}

fn check_id(field: &str, value: &str) -> Result<(), KitError> {
    if value.chars().count() > 128 {
        return Err(KitError::bad_request(field, "too_long").with_limit(128));
    }
    Ok(())
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "chat_cancel"), err(level = "warn"))]
pub(crate) fn chat_cancel<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    run_id: String,
) -> Result<Ack, KitError> {
    check_id("runId", &run_id)?;
    let runs = app.state::<Runs>();
    runs.service.cancel(&runs.owner(window.label())?, &run_id)
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "chat_approve"), err(level = "warn"))]
pub(crate) fn chat_approve<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    run_id: String,
    call_id: String,
    allow: bool,
) -> Result<Ack, KitError> {
    check_id("runId", &run_id)?;
    check_id("callId", &call_id)?;
    let runs = app.state::<Runs>();
    runs.service
        .approve(&runs.owner(window.label())?, &run_id, &call_id, allow)
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "chat_snapshot"), err(level = "warn"))]
pub(crate) fn chat_snapshot<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    run_id: String,
) -> Result<RunSnapshot, KitError> {
    check_id("runId", &run_id)?;
    let runs = app.state::<Runs>();
    runs.service.snapshot(&runs.owner(window.label())?, &run_id)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use super::*;

    struct Host;
    impl aether_runtime::KitHost for Host {}

    #[tokio::test]
    async fn destroyed_window_is_not_recreated_by_lookup_and_same_label_gets_new_owner() {
        let root = tempfile::tempdir().unwrap();
        let providers = Arc::new(aether_providers::Store::open(root.path()).unwrap());
        let service = RunService::new(
            Arc::new(Host),
            aether_runtime::RunConfig::default(),
            aether_runtime::RunDependencies::new(providers),
            aether_support::net::AddressPolicy::default(),
        )
        .unwrap();
        let runs = Runs::new(service);
        runs.register("main");
        let old = runs.owner("main").unwrap();
        runs.close("main");
        assert!(runs.owner("main").is_err());
        runs.register("main");
        let current = runs.owner("main").unwrap();
        let request = || {
            serde_json::from_value(serde_json::json!({
            "purpose": "chat", "skills": [], "input": "hello", "regenerate": false, "language": "en"
        })).unwrap()
        };
        let closed = runs
            .service
            .start(
                RunContext {
                    owner: old,
                    own_mcp_port: None,
                },
                request(),
                Arc::new(|_| Ok(())),
            )
            .await
            .unwrap_err();
        assert_eq!(closed.code(), aether_support::KitErrorCode::Cancelled);
        let open = runs
            .service
            .start(
                RunContext {
                    owner: current,
                    own_mcp_port: None,
                },
                request(),
                Arc::new(|_| Ok(())),
            )
            .await
            .unwrap_err();
        assert_eq!(open.code(), aether_support::KitErrorCode::NeedsConfig);
        runs.service
            .shutdown(std::time::Duration::from_secs(3))
            .await;
    }
}
