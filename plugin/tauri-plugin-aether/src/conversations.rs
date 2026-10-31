//! Independent history IPC; all domain validation and UI projection belong
//! to runtime.

use std::sync::Arc;

use aether_runtime::Ack;
use aether_runtime::contract::conversation_dto::{
    self as dto, UiConversation, UiConversationSummary,
};
use aether_runtime::conversations::ConversationService;
use aether_support::KitError;
use tauri::{AppHandle, Manager, Runtime};

async fn blocking<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&ConversationService) -> Result<T, KitError>
        + Send
        + 'static,
) -> Result<T, KitError> {
    let service = app.state::<Arc<ConversationService>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(&service))
        .await
        .map_err(KitError::internal)?
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_conversations"),
    err(level = "warn")
)]
pub(crate) async fn chat_conversations<R: Runtime>(
    app: AppHandle<R>,
    scope: Option<String>,
) -> Result<Vec<UiConversationSummary>, KitError> {
    blocking(&app, move |service| {
        service
            .list(scope.as_deref())
            .map(|items| items.into_iter().map(dto::summary).collect())
    })
    .await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_conversation"),
    err(level = "warn")
)]
pub(crate) async fn chat_conversation<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<UiConversation, KitError> {
    blocking(&app, move |service| service.get(&id).map(dto::conversation)).await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_delete"),
    err(level = "warn")
)]
pub(crate) async fn chat_delete<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<Ack, KitError> {
    blocking(&app, move |service| service.delete(&id)).await?;
    Ok(Ack { ok: true })
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_rename"),
    err(level = "warn")
)]
pub(crate) async fn chat_rename<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    title: String,
) -> Result<UiConversationSummary, KitError> {
    blocking(&app, move |service| {
        service.rename(&id, &title).map(dto::summary)
    })
    .await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_edit_turn"),
    err(level = "warn")
)]
pub(crate) async fn chat_edit_turn<R: Runtime>(
    app: AppHandle<R>,
    conversation_id: String,
    turn_id: String,
    text: String,
    expected_text: String,
) -> Result<UiConversation, KitError> {
    blocking(&app, move |service| {
        service
            .edit_turn(&conversation_id, &turn_id, &text, &expected_text)
            .map(dto::conversation)
    })
    .await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_remove_turn"),
    err(level = "warn")
)]
pub(crate) async fn chat_remove_turn<R: Runtime>(
    app: AppHandle<R>,
    conversation_id: String,
    turn_id: String,
) -> Result<UiConversation, KitError> {
    blocking(&app, move |service| {
        service
            .remove_turn(&conversation_id, &turn_id)
            .map(dto::conversation)
    })
    .await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_fork"),
    err(level = "warn")
)]
pub(crate) async fn chat_fork<R: Runtime>(
    app: AppHandle<R>,
    conversation_id: String,
    turn_id: String,
) -> Result<UiConversation, KitError> {
    blocking(&app, move |service| {
        service
            .fork(&conversation_id, &turn_id)
            .map(dto::conversation)
    })
    .await
}

#[tauri::command]
#[tracing::instrument(
    skip_all,
    fields(command = "chat_select_turn"),
    err(level = "warn")
)]
pub(crate) async fn chat_select_turn<R: Runtime>(
    app: AppHandle<R>,
    conversation_id: String,
    turn_id: String,
) -> Result<UiConversation, KitError> {
    blocking(&app, move |service| {
        service
            .select_turn(&conversation_id, &turn_id)
            .map(dto::conversation)
    })
    .await
}
