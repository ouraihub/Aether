//! Adapter feature validation; run use-case coverage lives in aether-runtime.
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use crate::Builder;
use aether_observability::{LogConfig, LogHandle};
use std::sync::LazyLock;

static LOGS: LazyLock<(tempfile::TempDir, LogHandle)> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    let logs = aether_observability::init(
        dir.path(),
        LogConfig::default(),
        None,
    )
    .unwrap();
    (dir, logs)
});

#[test]
fn enabled_adapters_require_their_own_instances() {
    let result = Builder::new(LOGS.1.clone()).build::<tauri::Wry>();
    let expected = if cfg!(feature = "chat") {
        Some("runs")
    } else if cfg!(feature = "conversations") {
        Some("conversations")
    } else if cfg!(feature = "providers") {
        Some("providers")
    } else if cfg!(feature = "mcp-client") {
        Some("mcp-client")
    } else if cfg!(feature = "mcp-server") {
        Some("mcp-server")
    } else if cfg!(feature = "skills") {
        Some("skills")
    } else if cfg!(feature = "search") {
        Some("search")
    } else {
        None
    };
    if let Some(field) = expected {
        let error = result.err().unwrap();
        assert_eq!(error.code(), aether_support::KitErrorCode::BadRequest);
        assert_eq!(error.details().field(), Some(field));
        assert_eq!(error.details().reason(), Some("missing_instance"));
    } else {
        assert!(result.is_ok());
    }
}

#[cfg(all(
    feature = "providers",
    not(any(
        feature = "chat",
        feature = "conversations",
        feature = "skills",
        feature = "search",
        feature = "mcp-client",
        feature = "mcp-server"
    ))
))]
#[test]
fn providers_only_needs_no_host_or_run_service() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        aether_providers::Store::open(dir.path()).unwrap(),
    );
    assert!(
        Builder::new(LOGS.1.clone())
            .providers(store)
            .build::<tauri::Wry>()
            .is_ok()
    );
}

#[cfg(all(
    feature = "conversations",
    not(any(
        feature = "chat",
        feature = "providers",
        feature = "skills",
        feature = "search",
        feature = "mcp-client",
        feature = "mcp-server"
    ))
))]
#[test]
fn history_only_needs_no_host_or_run_service() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        aether_runtime::conversations::SqliteStore::open(dir.path())
            .unwrap(),
    );
    let service = std::sync::Arc::new(
        aether_runtime::conversations::ConversationService::new(store),
    );
    assert!(
        Builder::new(LOGS.1.clone())
            .conversations(service)
            .build::<tauri::Wry>()
            .is_ok()
    );
}
