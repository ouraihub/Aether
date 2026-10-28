//! 示例宿主：只启用 providers。验收用法见 `docs/90-开发任务清单.md`。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use aether_observability::LogConfig;

#[expect(
    clippy::print_stderr,
    reason = "logging may not be initialised yet; this is the only way to report a startup failure"
)]
fn main() {
    if let Err(error) = run() {
        eprintln!("example-providers-only failed to start: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Before `tauri::Builder`, so logs from other plugins' setup reach the file (docs/10 §8).
    let root = aether_observability::data_root("aether-providers-only")?;
    let logs = aether_observability::init(&root.join("logs"), LogConfig::default(), None)?;
    let aether = tauri_plugin_aether::Builder::new(logs)
        .providers(Arc::new(aether_providers::Store::open(&root)?))
        .build()?;
    tauri::Builder::default()
        .plugin(aether)
        .run(tauri::generate_context!())?;
    Ok(())
}
