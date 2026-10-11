//! Small contract types shared by crates that must not depend on each other:
//! `Approval` (chat + mcp-client) and `SkillRef` (skills + the integration layer
//! when the skills feature is off). Docs: 12 §2, 13 §2, 14 §5.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Default approval of a tool in the run catalogue; the host has the final say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Approval {
    /// Run without asking.
    Auto,
    /// Ask the user before every call.
    Ask,
    /// Never run.
    Deny,
}

/// A skill selected for one run, pinned to the revision the user confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SkillRef {
    /// Skill id.
    pub id: String,
    /// Revision the user saw; a mismatch is `conflict{reason:"skill_changed"}`.
    pub revision: u32,
}
