//! Skill DTOs and the run snapshot (docs/14 §5, §6). Requests reject unknown fields.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use aether_support::SkillRef;

/// Where a skill came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkillOrigin {
    /// Imported from a `SKILL.md`, directory or zip.
    Imported,
    /// Written by the user in the editor (including duplicated built-ins).
    Manual,
    /// Shipped with the host; read-only.
    Builtin,
}

/// List entry: metadata only, never the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SkillMeta {
    /// Skill id.
    pub id: String,
    /// Frontmatter `name` (unique slug).
    pub name: String,
    /// Frontmatter `description`.
    pub description: String,
    /// `None` = unversioned; never invented.
    pub version: Option<String>,
    /// Origin.
    pub origin: SkillOrigin,
    /// Incremented on every content change.
    pub revision: u32,
    /// Body length in characters.
    #[ts(type = "number")]
    pub body_chars: usize,
    /// Unix milliseconds.
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// A skill with its full text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SkillDto {
    /// Metadata.
    #[serde(flatten)]
    #[ts(flatten)]
    pub meta: SkillMeta,
    /// Body text.
    pub body: String,
    /// License text, when packaged.
    pub license: Option<String>,
    /// Uninterpreted frontmatter keys, shown as-is.
    pub extra: BTreeMap<String, String>,
}

/// Whether confirming a preview installs a new skill or replaces one with the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PreviewAction {
    /// No skill with this name exists.
    Install,
    /// A skill with this name exists; installing needs `replace = true`.
    Replace,
}

/// The installed skill a preview would replace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewCurrent {
    /// Installed version.
    pub version: Option<String>,
    /// Installed revision.
    pub revision: u32,
}

/// What the user confirms before install; `digest` must come back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Preview {
    /// `sha256(name|description|version|body)`.
    pub digest: String,
    /// Frontmatter `name`.
    pub name: String,
    /// Frontmatter `description`.
    pub description: String,
    /// Frontmatter `version`.
    pub version: Option<String>,
    /// Full body.
    pub body: String,
    /// License text.
    pub license: Option<String>,
    /// Uninterpreted frontmatter keys.
    pub extra: BTreeMap<String, String>,
    /// Install or replace.
    pub action: PreviewAction,
    /// Present when `action` is `replace`.
    pub current: Option<PreviewCurrent>,
}

/// Create (no `id`) or edit a hand-written skill.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ManualSave {
    /// Existing skill id when editing.
    #[serde(default)]
    #[ts(optional)]
    pub id: Option<String>,
    /// Must equal the current revision when editing (two-window guard).
    #[serde(default)]
    #[ts(optional)]
    pub expected_revision: Option<u32>,
    /// Slug name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Version; absent or `null` = unversioned.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub version: Option<String>,
    /// Body.
    pub body: String,
}

/// Skill text pinned at run start; later edits do not affect the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    /// Skill id.
    pub id: String,
    /// Pinned revision.
    pub revision: u32,
    /// Skill name.
    pub name: String,
    /// Body text.
    pub body: String,
}
