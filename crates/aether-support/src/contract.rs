//! Small contract types shared by crates that must not depend on each other:
//! `Approval` (chat + mcp-client), `PermissionMode` / `ToolEffect` (chat + the integration
//! layer) and `SkillRef` (skills + the integration layer when the skills feature is off).
//! Docs: 12 §2, §5.1, 13 §2, 14 §5.

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

/// How much of the tool catalogue one run may use without asking; chosen per run, fixed
/// once the run is accepted (docs/12 §5.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionMode {
    /// Every tool keeps its own approval.
    #[default]
    Default,
    /// Only `Read` tools are offered; nothing changes data.
    Plan,
    /// `Edit` tools run without asking.
    AcceptEdits,
    /// Host tools run without asking unless the host's rules ask again; MCP tools unchanged.
    Auto,
    /// Every `Ask` runs without asking, MCP tools included. `Deny` stays `Deny`.
    FullAccess,
}

impl PermissionMode {
    /// Modes every host offers unless it narrows the list; `Auto` and `FullAccess` are opt-in.
    pub const BASELINE: [Self; 3] = [Self::Default, Self::Plan, Self::AcceptEdits];
}

/// What a tool does, declared by the host; drives the permission mode matrix (docs/12 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToolEffect {
    /// Reads only; offered in `Plan`.
    Read,
    /// Changes the host's documents or data.
    Edit,
    /// Runs a program or script.
    Exec,
    /// Anything else, including every MCP tool (their read-only hints are not trusted).
    Other,
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
