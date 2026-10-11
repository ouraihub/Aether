//! `skills_*` commands (docs/15 §5, docs/14 §4–§6).

use std::sync::Arc;

use aether_skills::{ManualSave, Preview, SkillDto, SkillMeta, Source, Store};
use aether_support::KitError;
use tauri::{AppHandle, Manager, Runtime};

/// The store, managed as Tauri state; `chat_run` snapshots from it.
pub(crate) struct Skills {
    pub(crate) store: Arc<Store>,
}

/// Runs a blocking store call off the async runtime.
async fn blocking<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&Store) -> Result<T, KitError> + Send + 'static,
) -> Result<T, KitError> {
    let store = Arc::clone(&app.state::<Skills>().store);
    tauri::async_runtime::spawn_blocking(move || f(&store))
        .await
        .map_err(KitError::internal)?
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_list"), err(level = "warn"))]
pub(crate) async fn skills_list<R: Runtime>(app: AppHandle<R>) -> Result<Vec<SkillMeta>, KitError> {
    blocking(&app, Store::list).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_get"), err(level = "warn"))]
pub(crate) async fn skills_get<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<SkillDto, KitError> {
    blocking(&app, move |store| store.get(&id)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_preview"), err(level = "warn"))]
pub(crate) async fn skills_preview<R: Runtime>(
    app: AppHandle<R>,
    source: Source,
) -> Result<Preview, KitError> {
    blocking(&app, move |store| store.preview(&source)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_install"), err(level = "warn"))]
pub(crate) async fn skills_install<R: Runtime>(
    app: AppHandle<R>,
    source: Source,
    confirmed_digest: String,
    replace: bool,
) -> Result<Vec<SkillMeta>, KitError> {
    blocking(&app, move |store| {
        store.install(&source, &confirmed_digest, replace)
    })
    .await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_save"), err(level = "warn"))]
pub(crate) async fn skills_save<R: Runtime>(
    app: AppHandle<R>,
    input: ManualSave,
) -> Result<Vec<SkillMeta>, KitError> {
    blocking(&app, move |store| store.save_manual(&input)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_duplicate"), err(level = "warn"))]
pub(crate) async fn skills_duplicate<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    new_name: String,
) -> Result<Vec<SkillMeta>, KitError> {
    blocking(&app, move |store| store.duplicate(&id, &new_name)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_reorder"), err(level = "warn"))]
pub(crate) async fn skills_reorder<R: Runtime>(
    app: AppHandle<R>,
    ids: Vec<String>,
) -> Result<Vec<SkillMeta>, KitError> {
    blocking(&app, move |store| store.reorder(&ids)).await
}

#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "skills_delete"), err(level = "warn"))]
pub(crate) async fn skills_delete<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<Vec<SkillMeta>, KitError> {
    blocking(&app, move |store| store.delete(&id)).await
}
