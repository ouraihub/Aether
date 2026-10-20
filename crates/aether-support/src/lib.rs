//! Private workspace crate: errors, masking, ids, data root and the connection snapshot.
//! Hosts never depend on it directly; public crates re-export what hosts need.
//!
//! Design: `docs/10-详细设计-基础与可观测性.md` part 1.

pub mod connection;
pub mod contract;
pub mod db;
pub mod error;
pub mod id;
pub mod mask;
pub mod net;
pub mod paths;
pub mod pricing;
pub mod reasoning;
pub mod secret;
pub mod sse;

pub use connection::{Connection, Protocol};
pub use contract::{Approval, SkillRef};
pub use error::{Details, KitError, KitErrorCode};
pub use pricing::{Cost, Currency, ModelPricing, PricingTier, cost};
pub use reasoning::{
    ContextSource, ContextUsage, ReasoningBlock, ReasoningConfig, ReasoningDialect,
    ReasoningEffort, TokenUsage,
};
