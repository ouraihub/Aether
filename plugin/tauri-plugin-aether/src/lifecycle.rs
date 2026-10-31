use aether_observability::LogHandle;
use tauri::{Manager, Runtime};

/// Exit, unlike ExitRequested, cannot be vetoed by the host.
pub(crate) fn on_event<R: Runtime>(
    app: &tauri::AppHandle<R>,
    event: &tauri::RunEvent,
) {
    #[cfg(all(feature = "chat", feature = "skills", feature = "providers"))]
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = event
        && let Some(authoring) =
            app.try_state::<super::skill_authoring::SkillAuthoring>()
    {
        authoring.close(label);
    }
    #[cfg(feature = "skills")]
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = event
    {
        app.state::<super::skill_sources::SearchRoots>().close(label);
    }
    #[cfg(feature = "chat")]
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = event
    {
        app.state::<super::runs::Runs>().close(label);
        tracing::info!("aether.plugin.window_destroyed.cancelled");
    }
    if matches!(event, tauri::RunEvent::Exit) {
        tracing::info!("aether.plugin.shutdown.started");
        tauri::async_runtime::block_on(async {
            #[cfg(feature = "chat")]
            app.state::<super::runs::Runs>()
                .service
                .shutdown(std::time::Duration::from_secs(3))
                .await;
            #[cfg(all(
                feature = "chat",
                feature = "skills",
                feature = "providers",
            ))]
            if let Some(authoring) =
                app.try_state::<super::skill_authoring::SkillAuthoring>()
            {
                authoring
                    .service
                    .shutdown(std::time::Duration::from_secs(3))
                    .await;
            }
            #[cfg(feature = "mcp-client")]
            {
                let client = app.state::<super::mcp_client::McpClient>();
                client.cancel.cancel();
                client.runtime.shutdown().await;
                tracing::info!("aether.plugin.mcp_client.shutdown.complete");
            }
            #[cfg(feature = "mcp-server")]
            if let Err(error) =
                app.state::<aether_mcp_server::McpServer>().stop().await
            {
                tracing::warn!(code = ?error.code(), "mcp_server.stop_failed");
            }
            #[cfg(feature = "providers")]
            app.state::<super::providers::Providers>().cancel_all();
            #[cfg(feature = "search")]
            app.state::<super::search::Search>().cancel_all();
            tracing::info!("aether.plugin.shutdown.finished");
            if let Err(error) = app.state::<LogHandle>().flush() {
                tracing::warn!(%error, "logs.flush_failed");
            }
        });
    }
}
