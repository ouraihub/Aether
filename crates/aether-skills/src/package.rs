//! Reading a skill package from a `SKILL.md`, a directory or a ZIP (docs/14 §2.2).
//! Everything stays in memory; sizes are counted on the bytes actually read, never
//! taken from ZIP headers (docs/14 §7.3).

use std::fs::{self, File};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use aether_support::{KitError, mask};
use serde::Deserialize;
use ts_rs::TS;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::frontmatter::{self, SkillFile};

const MAX_ZIP: usize = 512 * 1024;
const MAX_ENTRIES: usize = 8;
const MAX_SKILL: usize = 64 * 1024;
const MAX_LICENSE: usize = 32 * 1024;
// The 256 KiB cumulative cap of docs/14 §2.2 holds by construction: only SKILL.md
// (64 KiB) and license files (32 KiB each) are decompressed, and a second license is
// rejected as `duplicate` right after it is read, so at most ~128 KiB is ever inflated.

/// Where a skill package comes from. JSON: `{ "path": "C:\\…" }` or
/// `{ "bytes": { "fileName": "x.zip", "bytes": [80, 75, …] } }`.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Source {
    /// Absolute path to a `SKILL.md`, a directory or a `.zip`.
    Path(PathBuf),
    /// Dropped file content; `file_name` ending in `.zip` selects ZIP, anything else is `SKILL.md`.
    #[serde(rename_all = "camelCase")]
    Bytes {
        /// Original file name.
        file_name: String,
        /// File content.
        bytes: Vec<u8>,
    },
}

/// A validated package.
#[derive(Debug)]
pub(crate) struct Package {
    pub(crate) skill: SkillFile,
    pub(crate) license: Option<String>,
}

/// Reads and fully validates a package.
pub(crate) fn read(source: &Source) -> Result<Package, KitError> {
    match source {
        Source::Path(path) => read_path(path),
        Source::Bytes { file_name, bytes } if is_zip(Path::new(file_name)) => read_zip(bytes),
        Source::Bytes { bytes, .. } => {
            let mut files = Files::default();
            files.put("SKILL.md", bytes, MAX_SKILL)?;
            files.finish()
        }
    }
}

fn read_path(path: &Path) -> Result<Package, KitError> {
    if !path.is_absolute() {
        return Err(KitError::bad_request("path", "not_absolute"));
    }
    let meta = fs::symlink_metadata(path).map_err(not_found)?;
    if is_link(&meta) {
        return Err(KitError::bad_request("path", "link"));
    }
    if meta.is_dir() {
        return read_dir(path);
    }
    if is_zip(path) {
        let bytes = read_capped(File::open(path)?, MAX_ZIP)?;
        return read_zip(&bytes);
    }
    let mut files = Files::default();
    files.put(
        "SKILL.md",
        &read_capped(File::open(path)?, MAX_SKILL)?,
        MAX_SKILL,
    )?;
    files.finish()
}

/// Top level only; links (including Windows junctions) are refused, not followed.
fn read_dir(dir: &Path) -> Result<Package, KitError> {
    let mut files = Files::default();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = fs::symlink_metadata(&path)?;
        if is_link(&meta) {
            return Err(KitError::bad_request(&name, "link"));
        }
        match limit_for(&name) {
            Some(limit) if meta.is_file() => {
                files.put(&name, &read_capped(File::open(&path)?, limit)?, limit)?;
            }
            _ if meta.is_dir() => files.unsupported.push(format!("{name}/")),
            _ => files.unsupported.push(name),
        }
    }
    files.finish()
}

fn read_zip(bytes: &[u8]) -> Result<Package, KitError> {
    if bytes.len() > MAX_ZIP {
        return Err(too_large("zip", MAX_ZIP));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(invalid_zip)?;
    if archive.len() > MAX_ENTRIES {
        return Err(too_large("entries", MAX_ENTRIES));
    }

    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(invalid_zip)?;
        if entry.encrypted() {
            return Err(KitError::bad_request("zip", "encrypted"));
        }
        if entry.is_symlink() {
            return Err(KitError::bad_request("zip", "link"));
        }
        if !entry.is_dir() {
            entries.push((index, normalize(entry.name())?));
        }
    }
    let mut seen: Vec<&str> = entries.iter().map(|(_, p)| p.as_str()).collect();
    seen.sort_unstable();
    if seen.windows(2).any(|w| w[0] == w[1]) {
        return Err(KitError::bad_request("zip", "duplicate_path"));
    }

    // Accept the files at the root or inside one shared top-level directory.
    let prefix = entries
        .first()
        .and_then(|(_, p)| p.split_once('/'))
        .map(|(top, _)| format!("{top}/"));
    let strip = prefix
        .filter(|prefix| entries.iter().all(|(_, p)| p.starts_with(prefix)))
        .map_or(0, |prefix| prefix.len());

    let mut files = Files::default();
    for (index, path) in &entries {
        let name = &path[strip..];
        match limit_for(name) {
            Some(limit) => {
                let entry = archive.by_index(*index).map_err(invalid_zip)?;
                let data = read_capped(entry, limit).map_err(|e| invalid_zip(e.into()))?;
                files.put(name, &data, limit)?;
            }
            None => files.unsupported.push(name.to_owned()),
        }
    }
    files.finish()
}

/// Collapses `.` and empty segments so `./SKILL.md` and `SKILL.md` count as duplicates.
fn normalize(name: &str) -> Result<String, KitError> {
    if name.contains('\\') {
        return Err(KitError::bad_request("zip", "backslash_path"));
    }
    if name.starts_with('/') || name.contains(':') {
        return Err(KitError::bad_request("zip", "absolute_path"));
    }
    let parts: Vec<&str> = name
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.contains(&"..") {
        return Err(KitError::bad_request("zip", "path_traversal"));
    }
    Ok(parts.join("/"))
}

#[derive(Default)]
struct Files {
    skill: Option<String>,
    license: Option<String>,
    unsupported: Vec<String>,
}

impl Files {
    fn put(&mut self, name: &str, bytes: &[u8], limit: usize) -> Result<(), KitError> {
        if bytes.len() > limit {
            return Err(too_large(name, limit));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| KitError::bad_request(name, "not_utf8"))?
            .to_owned();
        // Report the line only; the matched text never reaches errors or logs (docs/14 §2.2).
        if let Some((line, _)) = mask::find_secret(&text) {
            return Err(KitError::bad_request(
                &format!("{name}:{line}"),
                "secret_detected",
            ));
        }
        let slot = if name == "SKILL.md" {
            &mut self.skill
        } else {
            &mut self.license
        };
        if slot.replace(text).is_some() {
            return Err(KitError::bad_request(name, "duplicate"));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Package, KitError> {
        if !self.unsupported.is_empty() {
            self.unsupported.sort();
            return Err(KitError::bad_request(
                &self.unsupported.join(", "),
                "unsupported_files",
            ));
        }
        let skill = self
            .skill
            .ok_or_else(|| KitError::bad_request("SKILL.md", "missing"))?;
        Ok(Package {
            skill: frontmatter::parse(&skill)?,
            license: self.license,
        })
    }
}

fn limit_for(name: &str) -> Option<usize> {
    match name {
        "SKILL.md" => Some(MAX_SKILL),
        "LICENSE" | "LICENSE.md" | "LICENSE.txt" => Some(MAX_LICENSE),
        _ => None,
    }
}

/// Reads at most `limit + 1` bytes so an oversize input is detected without reading it all.
fn read_capped(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut data)?;
    Ok(data)
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

/// Junctions are reparse points that `is_symlink` may not report (docs/14 §7.4).
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

fn too_large(field: &str, limit: usize) -> KitError {
    KitError::bad_request(field, "too_large").with_limit(limit as u64)
}

fn invalid_zip(error: ZipError) -> KitError {
    KitError::bad_request("zip", "invalid").with_cause(error)
}

fn not_found(error: io::Error) -> KitError {
    if error.kind() == io::ErrorKind::NotFound {
        KitError::not_found("skill_source")
    } else {
        error.into()
    }
}

#[cfg(test)]
mod tests;
