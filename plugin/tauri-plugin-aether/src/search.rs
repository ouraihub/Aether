//! `search_*` commands and cancellation jobs.
use aether_search::{
    CheckResult, SearchCap, SearchPage, SearchResult, SearchSave, SearchSettings, Store,
};
use aether_support::KitError;
use std::{collections::HashMap, sync::Arc};
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

pub(crate) struct Search {
    pub store: Arc<Store>,
    pub jobs: std::sync::Mutex<HashMap<String, CancellationToken>>,
}
impl Search {
    pub(crate) fn from_arc(store: Arc<Store>) -> Self {
        Self {
            store,
            jobs: std::sync::Mutex::new(HashMap::new()),
        }
    }
}
fn start<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<CancellationToken, KitError> {
    if id.len() > 64 {
        return Err(KitError::bad_request("jobId", "too_long"));
    }
    let token = CancellationToken::new();
    app.state::<Search>()
        .jobs
        .lock()
        .map_err(|_| KitError::internal(std::io::Error::other("search jobs lock poisoned")))?
        .insert(id.into(), token.clone());
    Ok(token)
}
fn finish<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if let Ok(mut jobs) = app.state::<Search>().jobs.lock() {
        jobs.remove(id);
    }
}
#[tauri::command]
pub(crate) async fn search_settings<R: Runtime>(
    app: AppHandle<R>,
    capability: SearchCap,
) -> Result<SearchSettings, KitError> {
    let s = app.state::<Search>().store.clone();
    tauri::async_runtime::spawn_blocking(move || s.settings(capability))
        .await
        .map_err(KitError::internal)?
}
#[tauri::command]
pub(crate) async fn search_save<R: Runtime>(
    app: AppHandle<R>,
    input: SearchSave,
) -> Result<SearchSettings, KitError> {
    let s = app.state::<Search>().store.clone();
    tauri::async_runtime::spawn_blocking(move || s.save(input))
        .await
        .map_err(KitError::internal)?
}
#[tauri::command]
pub(crate) async fn search_set_default<R: Runtime>(
    app: AppHandle<R>,
    capability: SearchCap,
    provider: String,
    max_results: Option<u8>,
) -> Result<SearchSettings, KitError> {
    let s = app.state::<Search>().store.clone();
    tauri::async_runtime::spawn_blocking(move || s.set_default(capability, &provider, max_results))
        .await
        .map_err(KitError::internal)?
}
#[tauri::command]
pub(crate) async fn search_reveal<R: Runtime>(
    app: AppHandle<R>,
    capability: SearchCap,
    provider: String,
    _current_password: Option<String>,
) -> Result<String, KitError> {
    let s = app.state::<Search>().store.clone();
    tauri::async_runtime::spawn_blocking(move || s.reveal(capability, &provider))
        .await
        .map_err(KitError::internal)?
}
#[tauri::command]
pub(crate) async fn search_check<R: Runtime>(
    app: AppHandle<R>,
    capability: SearchCap,
    provider: String,
    job_id: String,
) -> Result<CheckResult, KitError> {
    let token = start(&app, &job_id)?;
    let s = app.state::<Search>().store.clone();
    let result = tokio::select! {r=s.check(capability,&provider)=>r,_=token.cancelled()=>Err(KitError::cancelled())};
    finish(&app, &job_id);
    result
}
#[tauri::command]
pub(crate) async fn search_query<R: Runtime>(
    app: AppHandle<R>,
    query: String,
    job_id: String,
) -> Result<SearchResult, KitError> {
    let token = start(&app, &job_id)?;
    let s = app.state::<Search>().store.clone();
    let result =
        tokio::select! {r=s.search(&query)=>r,_=token.cancelled()=>Err(KitError::cancelled())};
    finish(&app, &job_id);
    result
}
#[tauri::command]
pub(crate) async fn search_fetch<R: Runtime>(
    app: AppHandle<R>,
    url: String,
    job_id: String,
) -> Result<SearchPage, KitError> {
    let token = start(&app, &job_id)?;
    let s = app.state::<Search>().store.clone();
    let result =
        tokio::select! {r=s.fetch(&url)=>r,_=token.cancelled()=>Err(KitError::cancelled())};
    finish(&app, &job_id);
    result
}
#[tauri::command]
pub(crate) async fn search_cancel<R: Runtime>(
    app: AppHandle<R>,
    job_id: String,
) -> Result<(), KitError> {
    if let Ok(mut jobs) = app.state::<Search>().jobs.lock()
        && let Some(t) = jobs.remove(&job_id)
    {
        t.cancel();
    }
    Ok(())
}
