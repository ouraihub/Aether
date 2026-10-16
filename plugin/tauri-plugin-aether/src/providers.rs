//! `providers_*` commands (docs/15 §5, docs/11 §3–§5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use aether_providers::{
    ModelFilter, ModelOption, ProbeResult, ProbeTarget, Provider, ProviderSave, Store,
};
use aether_support::KitError;
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

/// The store plus active probe jobs by frontend-chosen id.
pub(crate) struct Providers {
    pub(crate) store: Arc<Store>,
    jobs: Mutex<HashMap<String, CancellationToken>>,
}

impl Providers {
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store: Arc::new(store),
            jobs: Mutex::default(),
        }
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        // A poisoned map only means a command panicked mid-update; entries are still valid.
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Longest accepted job id (the frontend sends a UUID).
const MAX_JOB_ID: usize = 64;

/// Runs a blocking store call off the async runtime.
async fn blocking<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&Store) -> Result<T, KitError> + Send + 'static,
) -> Result<T, KitError> {
    let store = Arc::clone(&app.state::<Providers>().store);
    tauri::async_runtime::spawn_blocking(move || f(&store))
        .await
        .map_err(KitError::internal)?
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "providers_list"), err(level = "warn"))]
pub(crate) async fn providers_list<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<Provider>, KitError> {
    blocking(&app, Store::list).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "providers_save"), err(level = "warn"))]
pub(crate) async fn providers_save<R: Runtime>(
    app: AppHandle<R>,
    input: ProviderSave,
) -> Result<Vec<Provider>, KitError> {
    blocking(&app, move |store| store.save(input)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "providers_delete"), err(level = "warn"))]
pub(crate) async fn providers_delete<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<Vec<Provider>, KitError> {
    blocking(&app, move |store| store.delete(&id)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "providers_key_reveal"), err(level = "warn"))]
pub(crate) async fn providers_key_reveal<R: Runtime>(
    app: AppHandle<R>,
    endpoint_id: String,
) -> Result<String, KitError> {
    blocking(&app, move |store| {
        Ok(store.endpoint_connection(&endpoint_id)?.api_key.map(|key| key.to_string()).ok_or_else(|| KitError::needs_config("missing_key"))?)
    }).await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "providers_model_options"),
    err(level = "warn")
)]
pub(crate) async fn providers_model_options<R: Runtime>(
    app: AppHandle<R>,
    filter: ModelFilter,
) -> Result<Vec<ModelOption>, KitError> {
    blocking(&app, move |store| store.model_options(filter)).await
}

/// Fetches the model list; `providers_cancel(jobId)` aborts it with `cancelled`.
#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "providers_probe"), err(level = "warn"))]
pub(crate) async fn providers_probe<R: Runtime>(
    app: AppHandle<R>,
    job_id: String,
    target: ProbeTarget,
) -> Result<ProbeResult, KitError> {
    if job_id.is_empty() || job_id.len() > MAX_JOB_ID {
        return Err(KitError::bad_request("jobId", "out_of_range"));
    }
    let state = app.state::<Providers>();
    let cancel = CancellationToken::new();
    if state
        .jobs()
        .insert(job_id.clone(), cancel.clone())
        .is_some()
    {
        return Err(KitError::bad_request("jobId", "duplicate"));
    }
    let result = state.store.probe(target, &cancel).await;
    state.jobs().remove(&job_id);
    result
}

/// Cancels a probe; unknown ids are fine (the probe may have just finished).
#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "providers_cancel"), err(level = "warn"))]
pub(crate) async fn providers_cancel<R: Runtime>(
    app: AppHandle<R>,
    job_id: String,
) -> Result<(), KitError> {
    if let Some(cancel) = app.state::<Providers>().jobs().remove(&job_id) {
        cancel.cancel();
    }
    Ok(())
}
