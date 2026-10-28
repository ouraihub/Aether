//! 服务商 / 线路 / 模型配置、线路多密钥（轮换 + 失败切换）、模型探测与能力推断、解析后端连接快照。不含角色与回落。
//!
//! 设计：`docs/11-详细设计-kit-providers.md`；显式连接准备与探测不要求 Store。

pub mod connection;
mod dto;
mod infer;
pub mod probe;
mod store;
mod url;

pub use aether_support::{
    Connection, ContextSource, ContextUsage, Cost, Currency, KitError, KitErrorCode, ModelPricing,
    PricingTier, Protocol, ReasoningBlock, ReasoningConfig, ReasoningDialect, ReasoningEffort,
    TokenUsage,
};
pub use dto::*;
pub use infer::{Inferred, infer, infer_reasoning};
pub use store::Store;
pub use url::api_base;

/// Native at-rest byte protection for host credentials, without a private support dependency.
pub mod secrets {
    pub use aether_support::secret::{protect, unprotect};
}
