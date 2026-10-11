//! Global `init` in its own test process: logs from another module/thread (as a plugin's
//! setup would emit) and from the `log` crate reach the file and read back via `tail`.
#![expect(clippy::unwrap_used, reason = "tests")]

use aether_observability::{LogConfig, LogLevel, init};

mod other_plugin {
    pub(crate) fn setup() {
        tracing::info!(plugin = "store", "plugin.setup.done");
        tracing_log::log::warn!("legacy log crate line");
    }
}

#[test]
fn init_captures_logs_from_other_modules_and_threads() {
    let dir = tempfile::tempdir().unwrap();
    let handle = init(dir.path(), LogConfig::default(), None).unwrap();
    assert!(
        init(dir.path(), LogConfig::default(), None)
            .is_err_and(|e| e.code() == aether_support::KitErrorCode::Conflict)
    );

    std::thread::spawn(other_plugin::setup).join().unwrap();
    handle.flush().unwrap();

    let entries = handle.tail(50).unwrap();
    let setup = entries
        .iter()
        .find(|e| e.message.starts_with("plugin.setup.done"))
        .unwrap();
    assert_eq!(setup.level, "INFO");
    assert_eq!(setup.target, "init::other_plugin");
    assert_eq!(setup.message, "plugin.setup.done plugin=\"store\"");
    let legacy = entries
        .iter()
        .find(|e| e.message == "legacy log crate line")
        .unwrap();
    assert_eq!(
        (legacy.level.as_str(), legacy.target.as_str()),
        ("WARN", "init::other_plugin")
    );

    handle.set_level(LogLevel::WARN).unwrap();
    tracing::info!("filtered.after.set_level");
    assert!(
        handle
            .tail(50)
            .unwrap()
            .iter()
            .all(|e| e.message != "filtered.after.set_level")
    );
    let persisted = std::fs::read_to_string(dir.path().join("level.json")).unwrap();
    assert_eq!(persisted, r#"{"level":"WARN"}"#);
    assert_eq!(handle.path(), dir.path().join("app.log"));

    #[expect(clippy::panic, reason = "exercises the panic hook")]
    let panicked = std::thread::spawn(|| panic!("boom in worker")).join();
    assert!(panicked.is_err());
    let last = handle.tail(1).unwrap();
    assert_eq!(last[0].level, "ERROR");
    assert!(
        last[0].message.starts_with("app.panic panic="),
        "{}",
        last[0].message
    );
    assert!(last[0].message.contains("boom in worker"));
}
