//! 对话执行、SSE 解析与工具循环。无接入层依赖。
//!
//! 设计：`docs/12-详细设计-kit-chat.md`。实现按 `docs/90-开发任务清单.md` 领取任务后填充。

#[cfg(feature = "native")]
mod native;
pub mod protocol;
pub mod risk;
#[cfg(feature = "native")]
mod run;
mod tools;
mod types;
mod validate;

pub use tokio_util::sync::CancellationToken;

/// Address restrictions for the Native engine.
#[cfg(feature = "native")]
pub use aether_support::net::AddressPolicy;
#[cfg(feature = "native")]
pub use run::RunEngine;
pub use tools::{Approver, AuthorizeContext, BoxFuture, ToolExecutor};
pub use types::*;
pub use validate::{check_images, validate};

#[cfg(all(test, feature = "native"))]
mod tests;
