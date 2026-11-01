//! Workspace file operations and path validation shared by the authoring tools.
use super::{
    AuthoringTools, MAX_FILE, MAX_OUTPUT, MAX_SKILL, MAX_VISITED, error, io_error,
    symlink_metadata, truncate,
};
use crate::{ToolError, ToolOutput};
use serde_json::json;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

impl AuthoringTools {
    pub(super) fn list(&self, raw: Option<&str>) -> Result<ToolOutput, ToolError> {
        let dir = self.path(raw.unwrap_or(""), false)?;
        let meta = symlink_metadata(&dir)?;
        if !meta.is_dir() {
            return Err(error("not_directory", "path is not a directory", false));
        }
        let mut rows = Vec::new();
        visit(&dir, &self.root, &mut rows, 0)?;
        rows.sort();
        // Text and payload share one byte budget; rows past it are dropped from both.
        let mut used = 0;
        let kept = rows
            .iter()
            .take_while(|row| {
                used += row.len() + 1;
                used <= MAX_OUTPUT - 4
            })
            .count();
        let truncated = kept < rows.len();
        rows.truncate(kept);
        let mut text = rows.join("\n");
        if truncated {
            text.push_str("\n…");
        }
        Ok(ToolOutput {
            text,
            payload: Some(json!({"files": rows, "truncated": truncated})),
        })
    }

    pub(super) fn read(&self, raw: &str, offset: usize) -> Result<ToolOutput, ToolError> {
        let path = self.path(raw, false)?;
        let meta = symlink_metadata(&path)?;
        if !meta.is_file() {
            return Err(error("not_file", "path is not a regular file", false));
        }
        let limit = if path.file_name().and_then(|v| v.to_str()) == Some("SKILL.md") {
            MAX_SKILL
        } else {
            MAX_FILE
        };
        if meta.len() as usize > limit {
            return Err(error("too_large", "file exceeds size limit", false));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        File::open(&path)
            .map_err(|e| io_error(&e))?
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error(&e))?;
        if bytes.len() > limit {
            return Err(error("too_large", "file exceeds size limit", false));
        }
        let body = String::from_utf8(bytes)
            .map_err(|_| error("invalid_utf8", "file is not UTF-8 text", false))?;
        let start = body
            .char_indices()
            .nth(offset)
            .map_or(body.len(), |(at, _)| at);
        let mut text = body[start..].to_owned();
        let truncated = truncate(&mut text, MAX_OUTPUT);
        let next = offset + text.trim_end_matches('…').chars().count();
        Ok(ToolOutput {
            text,
            payload: Some(json!({"path": raw, "truncated": truncated, "nextOffsetChars": next})),
        })
    }

    pub(super) fn write(&self, raw: &str, content: &str) -> Result<ToolOutput, ToolError> {
        let path = self.path(raw, true)?;
        let rel = path
            .strip_prefix(&self.root)
            .map_err(|_| error("path", "outside workspace", false))?;
        let mut parts = rel.components();
        let Some(Component::Normal(area)) = parts.next() else {
            return Err(error("path", "choose skill or evaluations", false));
        };
        let area = area.to_string_lossy();
        if area != "skill" && area != "evaluations" {
            return Err(error("path", "creator is read-only", false));
        }
        if area == "skill" && !allowed_skill_path(&parts.collect::<PathBuf>()) {
            return Err(error(
                "path",
                "skill file must be SKILL.md, LICENSE.txt, scripts, references, or assets",
                false,
            ));
        }
        let max = if path.file_name().and_then(|v| v.to_str()) == Some("SKILL.md") {
            MAX_SKILL
        } else {
            MAX_FILE
        };
        if content.len() > max {
            return Err(error("content", "file exceeds size limit", false));
        }
        let parent = path
            .parent()
            .ok_or_else(|| error("path", "invalid parent", false))?;
        ensure_write_parent(parent, &self.root)?;
        if path.exists() && !symlink_metadata(&path)?.is_file() {
            return Err(error("path", "target is not a regular file", false));
        }
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| io_error(&e))?;
        tmp.write_all(content.as_bytes())
            .map_err(|e| io_error(&e))?;
        tmp.as_file().sync_all().map_err(|e| io_error(&e))?;
        // Re-check just before the atomic replace; persist never opens the destination.
        ensure_dir_chain(parent, &self.root)?;
        if path.exists() {
            symlink_metadata(&path)?;
        }
        tmp.persist(&path).map_err(|e| io_error(&e.error))?;
        Ok(ToolOutput {
            text: format!("wrote {raw}"),
            payload: Some(json!({"path": raw, "bytes": content.len()})),
        })
    }

    pub(super) fn validate(&self) -> Result<ToolOutput, ToolError> {
        let skill = self.path("skill", false)?;
        ensure_dir_chain(&skill, &self.root)?;
        let prepared = aether_skills::system::prepare_directory(&skill)
            .map_err(|e| error("invalid_skill", &e.to_string(), false))?;
        let temp = tempfile::tempdir().map_err(|e| io_error(&e))?;
        let store = aether_skills::Store::open(temp.path(), &[])
            .map_err(|e| error("validation_failed", &e.to_string(), false))?;
        let preview = store
            .preview(&prepared.source)
            .map_err(|e| error("invalid_skill", &e.to_string(), false))?;
        let mut text = serde_json::to_string(&preview)
            .map_err(|_| error("validation_failed", "cannot serialize preview", false))?;
        truncate(&mut text, MAX_OUTPUT);
        Ok(ToolOutput {
            text,
            payload: Some(json!({"preview": preview, "ignored": prepared.ignored})),
        })
    }

    pub(super) fn path(&self, raw: &str, allow_missing: bool) -> Result<PathBuf, ToolError> {
        check_ancestors(&self.root)?;
        if raw.is_empty() {
            return Ok(self.root.clone());
        }
        let candidate = Path::new(raw);
        if raw.len() > 4096
            || raw.split('/').count() > 32
            || candidate.is_absolute()
            || raw.contains(['\0', '\\', ':'])
            || raw
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(error("path", "absolute or invalid path", false));
        }
        if !matches!(
            raw.split('/').next(),
            Some("creator" | "skill" | "evaluations")
        ) {
            return Err(error("path", "unknown workspace subtree", false));
        }
        let mut out = self.root.clone();
        for component in candidate.components() {
            let Component::Normal(part) = component else {
                return Err(error("path", "path traversal is not allowed", false));
            };
            let part = part
                .to_str()
                .ok_or_else(|| error("path", "path is not UTF-8", false))?;
            if is_device_name(part) {
                return Err(error("path", "device path is not allowed", false));
            }
            check_case(&out, part)?;
            out.push(part);
            match fs::symlink_metadata(&out) {
                Ok(_) => {
                    symlink_metadata(&out)?;
                }
                Err(e) if allow_missing && e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_error(&e)),
            }
        }
        if !allow_missing {
            ensure_dir_chain(out.parent().unwrap_or(&self.root), &self.root)?;
        }
        if !allow_missing || out.exists() {
            let _ = symlink_metadata(&out)?;
        }
        Ok(out)
    }
}

fn check_ancestors(path: &Path) -> Result<(), ToolError> {
    for ancestor in path.ancestors() {
        symlink_metadata(ancestor)?;
    }
    Ok(())
}

fn check_case(parent: &Path, name: &str) -> Result<(), ToolError> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_error(&e)),
    };
    for (visited, entry) in entries.enumerate() {
        if visited >= MAX_VISITED {
            return Err(error(
                "too_many_entries",
                "directory contains too many entries",
                false,
            ));
        }
        let entry = entry.map_err(|e| io_error(&e))?;
        let existing = entry.file_name();
        let existing = existing.to_string_lossy();
        if existing.to_lowercase() == name.to_lowercase() && existing != name {
            return Err(error("path", "case collision", false));
        }
    }
    Ok(())
}

fn ensure_write_parent(path: &Path, root: &Path) -> Result<(), ToolError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| error("path", "outside workspace", false))?;
    let mut current = root.to_owned();
    if !symlink_metadata(root)?.is_dir() {
        return Err(error("path", "workspace root is not a directory", false));
    }
    for part in relative.components() {
        let Component::Normal(part) = part else {
            return Err(error("path", "invalid path", false));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(_) if symlink_metadata(&current)?.is_dir() => {}
            Ok(_) => return Err(error("path", "ancestor is not a directory", false)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|e| io_error(&e))?
            }
            Err(e) => return Err(io_error(&e)),
        }
    }
    Ok(())
}

fn ensure_dir_chain(path: &Path, root: &Path) -> Result<(), ToolError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| error("path", "outside workspace", false))?;
    let mut current = root.to_owned();
    if !symlink_metadata(root)?.is_dir() {
        return Err(error("path", "workspace root is not a directory", false));
    }
    for part in relative.components() {
        let Component::Normal(part) = part else {
            return Err(error("path", "invalid path", false));
        };
        current.push(part);
        let meta = symlink_metadata(&current)?;
        if !meta.is_dir() {
            return Err(error("path", "ancestor is not a directory", false));
        }
    }
    Ok(())
}

fn allowed_skill_path(path: &Path) -> bool {
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(name))
            if name == "SKILL.md"
                || name == "LICENSE"
                || name == "LICENSE.md"
                || name == "LICENSE.txt" =>
        {
            components.next().is_none()
        }
        Some(Component::Normal(name))
            if name == "scripts" || name == "references" || name == "assets" =>
        {
            components.next().is_some()
        }
        _ => false,
    }
}

fn visit(path: &Path, root: &Path, rows: &mut Vec<String>, count: usize) -> Result<(), ToolError> {
    if count > 32 {
        return Err(error(
            "too_deep",
            "directory nesting exceeds 32 levels",
            false,
        ));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| io_error(&e))? {
        if entries.len() + rows.len() >= MAX_VISITED {
            return Err(error(
                "too_many_entries",
                "directory contains too many entries",
                false,
            ));
        }
        entries.push(entry.map_err(|e| io_error(&e))?);
    }
    entries.sort_by_key(|entry| entry.file_name());
    let mut names = std::collections::HashSet::new();
    for entry in entries {
        let file = entry.path();
        let meta = symlink_metadata(&file)?;
        let part = entry.file_name();
        let part = part
            .to_str()
            .ok_or_else(|| error("path", "path is not UTF-8", false))?;
        if is_device_name(part) {
            return Err(error("path", "invalid path name", false));
        }
        if !names.insert(part.to_lowercase()) {
            return Err(error("path", "case collision", false));
        }
        let rel = file
            .strip_prefix(root)
            .map_err(|_| error("path", "outside workspace", false))?;
        let name = rel.to_string_lossy().replace('\\', "/");
        if rows.len() >= MAX_VISITED {
            return Err(error(
                "too_many_entries",
                "directory contains too many entries",
                false,
            ));
        }
        rows.push(if meta.is_dir() {
            format!("{name}/")
        } else if meta.is_file() {
            name
        } else {
            format!("{name} [unsupported]")
        });
        if meta.is_dir() {
            visit(&file, root, rows, count + 1)?;
        }
    }
    Ok(())
}

fn is_device_name(name: &str) -> bool {
    if name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|ch| ch.is_control() || "<>:\\|?*".contains(ch))
    {
        return true;
    }
    let base = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        base.as_str(),
        "CON"
            | "CONIN$"
            | "CONOUT$"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    )
}
