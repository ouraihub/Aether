//! Read-only acquisition. Installation remains the store's digest-confirmed operation.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use aether_skills::sources::{PreparedSkill, RemoteSkill};
use aether_skills::system::SystemSkill;
use aether_support::KitError;
use tauri::{Manager, Runtime};

#[derive(Default)]
pub(crate) struct SearchRoots(Mutex<HashMap<String, Vec<PathBuf>>>);

impl SearchRoots {
    pub(crate) fn close(&self, label: &str) {
        if let Ok(mut roots) = self.0.lock() {
            roots.remove(label);
        }
    }
}

#[tauri::command]
pub(crate) async fn skills_source_search(query: String) -> Result<Vec<RemoteSkill>, KitError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        aether_skills::sources::search(&query),
    )
    .await
    .map_err(|_| KitError::bad_request("query", "timeout"))?
}

#[tauri::command]
pub(crate) async fn skills_source_prepare(url: String) -> Result<PreparedSkill, KitError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        aether_skills::sources::prepare(&url),
    )
    .await
    .map_err(|_| KitError::bad_request("url", "timeout"))?
}

#[tauri::command]
pub(crate) async fn skills_system_discover() -> Result<Vec<SystemSkill>, KitError> {
    tauri::async_runtime::spawn_blocking(aether_skills::system::discover)
        .await
        .map_err(KitError::internal)?
}

#[tauri::command]
pub(crate) async fn skills_system_prepare<R: Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    path: String,
) -> Result<PreparedSkill, KitError> {
    let roots = app
        .state::<SearchRoots>()
        .0
        .lock()
        .map_err(|_| KitError::internal(std::io::Error::other("skill roots unavailable")))?
        .get(window.label())
        .cloned()
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        for root in roots {
            let candidates = aether_skills::system::discover_directory(&root)?;
            if candidates.iter().any(|candidate| candidate.path == path) {
                return aether_skills::system::prepare_directory(std::path::Path::new(&path));
            }
        }
        aether_skills::system::prepare(&path)
    })
    .await
    .map_err(KitError::internal)?
}

#[tauri::command]
pub(crate) async fn skills_pick_directory() -> Result<Option<PreparedSkill>, KitError> {
    let Some(selected) = rfd::AsyncFileDialog::new().pick_folder().await else {
        return Ok(None);
    };
    let path = selected.path().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        aether_skills::system::prepare_directory(&path).map(Some)
    })
    .await
    .map_err(KitError::internal)?
}

#[tauri::command]
pub(crate) async fn skills_pick_search_directory<R: Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
) -> Result<Option<Vec<SystemSkill>>, KitError> {
    let Some(selected) = rfd::AsyncFileDialog::new().pick_folder().await else {
        return Ok(None);
    };
    let root = selected.path().to_owned();
    let scan_root = root.clone();
    let candidates = tauri::async_runtime::spawn_blocking(move || {
        aether_skills::system::discover_directory(&scan_root)
    })
    .await
    .map_err(KitError::internal)??;
    let state = app.state::<SearchRoots>();
    let mut all = state
        .0
        .lock()
        .map_err(|_| KitError::internal(std::io::Error::other("skill roots unavailable")))?;
    let roots = all.entry(window.label().to_owned()).or_default();
    if !roots.contains(&root) {
        if roots.len() >= 16 {
            return Err(KitError::bad_request("directory", "too_many"));
        }
        roots.push(root);
    }
    Ok(Some(candidates))
}
