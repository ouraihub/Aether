//! MCP 服务配置、凭据、信任、工具发现与调用、工具审批默认值。不依赖 chat。
//!
//! 设计：`docs/13-详细设计-kit-mcp.md 第一部分`。

mod dto;
mod protocol;
mod session;
mod store;
#[cfg(test)]
mod tests;

pub use dto::*;
pub use session::{McpCallResult, McpSession, McpTool};
pub use store::Store;
pub use tokio_util::sync::CancellationToken;
