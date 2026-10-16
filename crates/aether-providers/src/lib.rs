//! 服务商 / 线路 / 模型配置、线路多密钥（轮换 + 失败切换）、模型探测与能力推断、解析后端连接快照。不含角色与回落。
//!
//! 设计：`docs/11-详细设计-kit-providers.md`。实现按 `docs/90-开发任务清单.md` 领取任务后填充。

mod dto;
mod infer;
mod probe;
mod store;
mod url;

pub use aether_support::{
    Connection, ContextSource, ContextUsage, KitError, KitErrorCode, Protocol, ReasoningBlock,
    ReasoningConfig, ReasoningDialect, ReasoningEffort, TokenUsage,
};
pub use dto::*;
pub use infer::{Inferred, infer, infer_reasoning};
pub use store::Store;
pub use url::api_base;
