//! MCP 服务配置、凭据、信任、工具发现与调用、工具审批默认值；传输为 Streamable HTTP 或
//! 本地命令（stdio）。不依赖 chat。
//!
//! 设计：`docs/27-详细设计-MCP客户端.md`。

mod credentials;
mod dto;
mod http;
mod protocol;
mod runtime;
mod session;
mod stdio;
mod store;
#[cfg(test)]
mod tests;

pub use dto::*;
pub use runtime::McpRuntime;
pub use session::{McpCallResult, McpSession, McpTool};
pub use store::Store;
pub use tokio_util::sync::CancellationToken;
