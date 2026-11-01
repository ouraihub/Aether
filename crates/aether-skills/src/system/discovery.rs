use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use aether_support::KitError;

use super::{MAX_DEPTH, MAX_DISCOVERED, MAX_VISITED, is_link, too_large};

#[derive(Debug, Clone)]
pub(super) struct Root {
    pub(super) path: PathBuf,
    pub(super) origin: &'static str,
}

/// Discovers skills in the standard local agent roots.
///
/// # Errors
/// Rejects unavailable home, unreadable roots, links and discovery limits.
pub fn discover() -> Result<Vec<super::SystemSkill>, KitError> {
    let roots = known_roots()?;
    discover_roots(&roots)
}

/// Discovers skills below an explicitly selected directory.
///
/// # Errors
/// Rejects unsafe paths, unreadable directories and discovery limits.
pub fn discover_directory(root: &Path) -> Result<Vec<super::SystemSkill>, KitError> {
    let root = checked_root(root)?;
    discover_roots(&[Root {
        path: root,
        origin: "Selected folder",
    }])
}

/// Prepares a previously discovered skill directory for import.
///
/// # Errors
/// Rejects unregistered paths and invalid or oversized packages.
pub fn prepare(path: &str) -> Result<super::PreparedSkill, KitError> {
    let path = checked_input_path(Path::new(path))?;
    let candidates = discover()?;
    let candidate = candidates
        .iter()
        .find(|candidate| same_path(Path::new(&candidate.path), &path))
        .ok_or_else(|| KitError::bad_request("path", "not_discovered"))?;
    super::prepare_directory(Path::new(&candidate.path)).map(|mut prepared| {
        prepared.origin = candidate.origin.clone();
        prepared
    })
}

fn known_roots() -> Result<Vec<Root>, KitError> {
    let home = match env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => {
            return Err(KitError::internal(io::Error::new(
                io::ErrorKind::NotFound,
                "home unavailable",
            )));
        }
    };
    ensure_path_no_links(&home)?;
    let config = env_path(&home, "XDG_CONFIG_HOME", ".config");
    let claude = env_path(&home, "CLAUDE_CONFIG_DIR", ".claude");
    let codex = env_path(&home, "CODEX_HOME", ".codex");
    Ok(vec![
        Root {
            path: home.join(".agents/skills"),
            origin: "Agent Skills",
        },
        Root {
            path: config.join("agents/skills"),
            origin: "Agent Skills",
        },
        Root {
            path: claude.join("skills"),
            origin: "Claude Code",
        },
        Root {
            path: codex.join("skills"),
            origin: "Codex",
        },
        Root {
            path: home.join(".cursor/skills"),
            origin: "Cursor",
        },
        Root {
            path: home.join(".gemini/skills"),
            origin: "Gemini CLI",
        },
        Root {
            path: home.join(".copilot/skills"),
            origin: "GitHub Copilot",
        },
        Root {
            path: config.join("opencode/skills"),
            origin: "OpenCode",
        },
        Root {
            path: home.join(".omp/agent/skills"),
            origin: "OMP",
        },
        Root {
            path: home.join(".openclaw/skills"),
            origin: "OpenClaw",
        },
        Root {
            path: home.join(".clawdbot/skills"),
            origin: "ClawdBot",
        },
        Root {
            path: home.join(".moltbot/skills"),
            origin: "MoltBot",
        },
        Root {
            path: home.join(".qoder/skills"),
            origin: "Qoder",
        },
        Root {
            path: home.join(".qoder-cn/skills"),
            origin: "Qoder CN",
        },
        Root {
            path: home.join(".qwen/skills"),
            origin: "Qwen Code",
        },
        Root {
            path: home.join(".minimax/skills"),
            origin: "MiniMax Code",
        },
    ])
}

fn env_path(home: &Path, key: &str, fallback: &str) -> PathBuf {
    env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                home.join(path)
            }
        })
        .unwrap_or_else(|| home.join(fallback))
}

fn checked_root(path: &Path) -> Result<PathBuf, KitError> {
    let path = checked_input_path(path)?;
    let meta = fs::symlink_metadata(&path).map_err(KitError::internal)?;
    if !meta.is_dir() {
        return Err(KitError::bad_request("root", "not_directory"));
    }
    Ok(path)
}

fn ensure_path_no_links(path: &Path) -> Result<(), KitError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if is_link(&meta) => return Err(KitError::bad_request("path", "link")),
            Ok(_) => {}
            Err(error) => return Err(KitError::internal(error)),
        }
    }
    Ok(())
}

pub(super) fn checked_input_path(path: &Path) -> Result<PathBuf, KitError> {
    if !path.is_absolute() {
        return Err(KitError::bad_request("path", "not_absolute"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(KitError::bad_request("path", "traversal"));
    }
    ensure_path_no_links(path)?;
    let meta = fs::symlink_metadata(path).map_err(KitError::internal)?;
    if is_link(&meta) {
        return Err(KitError::bad_request("path", "link"));
    }
    Ok(path.to_path_buf())
}

fn discover_roots(roots: &[Root]) -> Result<Vec<super::SystemSkill>, KitError> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut visited = 0;
    for root in roots {
        let meta = match fs::symlink_metadata(&root.path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(KitError::internal(error)),
        };
        if is_link(&meta) {
            return Err(KitError::bad_request("root", "link"));
        }
        if !meta.is_dir() {
            continue;
        }
        ensure_path_no_links(&root.path)?;
        scan_dir(
            &root.path,
            root.origin,
            0,
            &mut result,
            &mut seen,
            &mut visited,
        )?;
    }
    Ok(result)
}

fn scan_dir(
    path: &Path,
    origin: &str,
    depth: usize,
    result: &mut Vec<super::SystemSkill>,
    seen: &mut HashSet<String>,
    visited: &mut usize,
) -> Result<(), KitError> {
    if depth > MAX_DEPTH {
        return Err(KitError::bad_request("path", "too_deep"));
    }
    let descriptor = path.join("SKILL.md");
    match fs::symlink_metadata(&descriptor) {
        Ok(meta) if is_link(&meta) => return Err(KitError::bad_request("SKILL.md", "link")),
        Ok(meta) if meta.is_file() => {
            let key = canonical_key(path)?;
            if seen.insert(key) {
                if result.len() >= MAX_DISCOVERED {
                    return Err(too_large("skills", MAX_DISCOVERED));
                }
                result.push(super::SystemSkill {
                    path: path.to_string_lossy().into_owned(),
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    origin: origin.to_owned(),
                });
            }
            return Ok(());
        }
        Ok(_) => return Err(KitError::bad_request("SKILL.md", "not_file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(KitError::internal(error)),
    }
    for entry in fs::read_dir(path).map_err(KitError::internal)? {
        *visited += 1;
        if *visited > MAX_VISITED {
            return Err(too_large("discovery", MAX_VISITED));
        }
        let child = entry.map_err(KitError::internal)?.path();
        let meta = fs::symlink_metadata(&child).map_err(KitError::internal)?;
        if is_link(&meta) {
            return Err(KitError::bad_request("path", "link"));
        }
        if meta.is_dir() {
            scan_dir(&child, origin, depth + 1, result, seen, visited)?;
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}

fn canonical_key(path: &Path) -> Result<String, KitError> {
    let value = fs::canonicalize(path)
        .map_err(KitError::internal)?
        .to_string_lossy()
        .into_owned();
    Ok(if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    })
}

pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || (cfg!(windows)
            && a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy()))
}
