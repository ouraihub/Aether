//! `logs_*` commands over the host's `LogHandle` (docs/15 §5, docs/10 §10). Always registered.
//! `logs_ingest` and `logs_tail` carry no span: span fields would be appended to every line.

use aether_observability::{
    DiagnosticLevel, IngestBatch, IngestReceipt, LogEntry, LogHandle, LogStatus,
};
use aether_support::KitError;
use tauri::{AppHandle, Manager, Runtime, State};

const MAX_TAIL: u16 = 2000;

/// The newest `limit` (≤ 2000) entries.
#[tauri::command]
pub(crate) async fn logs_tail<R: Runtime>(
    app: AppHandle<R>,
    limit: usize,
) -> Result<Vec<LogEntry>, KitError> {
    if limit > usize::from(MAX_TAIL) {
        return Err(KitError::bad_request("limit", "out_of_range").with_limit(MAX_TAIL.into()));
    }
    let logs = app.state::<LogHandle>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || logs.tail(limit))
        .await
        .map_err(KitError::internal)?
        .map_err(KitError::internal)
}

#[tauri::command]
pub(crate) async fn logs_get_level<R: Runtime>(app: AppHandle<R>) -> LogStatus {
    app.state::<LogHandle>().status()
}

/// Applies `level` immediately and persists it to `level.json`.
#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "logs_set_level"), err(level = "warn"))]
pub(crate) fn logs_set_level(
    logs: State<'_, LogHandle>,
    level: DiagnosticLevel,
) -> Result<LogStatus, KitError> {
    logs.set_level(level.into())?;
    Ok(logs.status())
}

#[tauri::command]
pub(crate) async fn logs_ingest<R: Runtime>(
    app: AppHandle<R>,
    batch: IngestBatch,
) -> Result<IngestReceipt, KitError> {
    let logs = app.state::<LogHandle>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || logs.ingest(&batch))
        .await
        .map_err(KitError::internal)
}

/// Opens the logs directory in the system file manager.
#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "logs_open_dir"), err(level = "warn"))]
pub(crate) async fn logs_open_dir<R: Runtime>(app: AppHandle<R>) -> Result<(), KitError> {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let Some(dir) = app
        .state::<LogHandle>()
        .path()
        .parent()
        .map(ToOwned::to_owned)
    else {
        return Err(KitError::not_found("logs directory"));
    };
    // The exit code is ignored: explorer returns 1 even after opening the window.
    tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new(opener).arg(dir).status()
    })
    .await
    .map_err(KitError::internal)?
    .map_err(KitError::internal)?;
    Ok(())
}
