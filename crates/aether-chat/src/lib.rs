//! 对话执行、SSE 解析与工具循环。无接入层依赖。
//!
//! 设计：`docs/12-详细设计-kit-chat.md`。实现按 `docs/90-开发任务清单.md` 领取任务后填充。

mod protocol;
mod run;
mod store;
mod tools;
mod types;
mod validate;

pub use tokio_util::sync::CancellationToken;

/// Hosts build a [`Runner`] with it without depending on the private `aether-support`.
pub use aether_support::net::AddressPolicy;
pub use run::{RunSummary, Runner, Usage};
pub use store::{
    Conversation, ConversationStore, ConversationSummary, SqliteStore, Turn, TurnHandle,
    TurnRecord, TurnStatus, history_path, translation_input, trim_history,
};
pub use tools::{Approver, BoxFuture, ToolExecutor};
pub use types::*;

#[cfg(test)]
mod tests;
