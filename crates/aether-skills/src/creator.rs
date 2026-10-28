//! Fixed skill-creator resources from an Apache-2.0 bundle; see `creator/LICENSE.txt`.
//! The runtime adapts filesystem locations and approvals; these files never execute on installation.
use std::path::Path;

use crate::{BuiltinSkill, KitError};

/// Trusted built-in creator, selected explicitly for a skill-authoring session.
pub static CREATOR_SKILLS: &[BuiltinSkill] = &[BuiltinSkill {
    name: "skill-creator",
    description: "Create, edit, evaluate and improve reusable skills with explicit user review.",
    version: Some("1.0.0"),
    body: include_str!("skill_creator.md"),
}];

/// Materializes fixed creator documentation and scripts in a new authoring workspace.
///
/// # Errors
/// Rejects an existing destination, or filesystem failures. No files are overwritten.
pub fn write_resources(destination: &Path) -> Result<(), KitError> {
    std::fs::create_dir(destination).map_err(KitError::internal)?;
    for (relative, bytes) in RESOURCES {
        let path = destination.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(KitError::internal)?;
        }
        std::fs::write(path, bytes).map_err(KitError::internal)?;
    }
    Ok(())
}

const RESOURCES: &[(&str, &[u8])] = &[
    ("SKILL.md", include_bytes!("creator/SKILL.md")),
    ("LICENSE.txt", include_bytes!("creator/LICENSE.txt")),
    (
        "agents/analyzer.md",
        include_bytes!("creator/agents/analyzer.md"),
    ),
    (
        "agents/comparator.md",
        include_bytes!("creator/agents/comparator.md"),
    ),
    (
        "agents/grader.md",
        include_bytes!("creator/agents/grader.md"),
    ),
    (
        "assets/eval_review.html",
        include_bytes!("creator/assets/eval_review.html"),
    ),
    (
        "eval-viewer/generate_review.py",
        include_bytes!("creator/eval-viewer/generate_review.py"),
    ),
    (
        "eval-viewer/viewer.html",
        include_bytes!("creator/eval-viewer/viewer.html"),
    ),
    (
        "references/schemas.md",
        include_bytes!("creator/references/schemas.md"),
    ),
    (
        "scripts/__init__.py",
        include_bytes!("creator/scripts/__init__.py"),
    ),
    (
        "scripts/aggregate_benchmark.py",
        include_bytes!("creator/scripts/aggregate_benchmark.py"),
    ),
    (
        "scripts/generate_report.py",
        include_bytes!("creator/scripts/generate_report.py"),
    ),
    (
        "scripts/improve_description.py",
        include_bytes!("creator/scripts/improve_description.py"),
    ),
    (
        "scripts/package_skill.py",
        include_bytes!("creator/scripts/package_skill.py"),
    ),
    (
        "scripts/quick_validate.py",
        include_bytes!("creator/scripts/quick_validate.py"),
    ),
    (
        "scripts/run_eval.py",
        include_bytes!("creator/scripts/run_eval.py"),
    ),
    (
        "scripts/run_loop.py",
        include_bytes!("creator/scripts/run_loop.py"),
    ),
    (
        "scripts/utils.py",
        include_bytes!("creator/scripts/utils.py"),
    ),
];
