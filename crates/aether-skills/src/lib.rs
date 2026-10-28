//! 技能导入、预览确认、修订、按次选择的快照。包内脚本与资源只保存并按修订读取，不执行、不落盘、不登记 MCP。
//!
//! 设计：`docs/14-详细设计-kit-skills.md`。实现按 `docs/90-开发任务清单.md` 领取任务后填充。

pub mod content;
mod dto;
mod frontmatter;
mod package;
mod store;

pub use aether_support::{KitError, KitErrorCode};
pub use dto::*;
pub use package::Source;
pub use store::{BuiltinSkill, Store};
