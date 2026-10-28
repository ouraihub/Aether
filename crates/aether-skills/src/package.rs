//! Reading a skill package from a `SKILL.md`, a directory or a ZIP (docs/14 §2).
//! Everything stays in memory; sizes are counted on the bytes actually read, never
//! taken from ZIP headers (docs/14 §7.3). Packaged files are data: nothing here runs
//! them or writes them to disk.

use std::fs::{self, File};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use aether_support::{KitError, mask};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::content::hex;
use crate::dto::SkillFileKind;
use crate::frontmatter::{self, SkillFile};

const MAX_ZIP: usize = 5 * 1024 * 1024;
/// ZIP entries, or files and directories visited when importing a directory.
const MAX_ENTRIES: usize = 200;
/// Bytes read across every file of one package.
const MAX_TOTAL: usize = 16 * 1024 * 1024;
const MAX_FILE: usize = 1024 * 1024;
const MAX_SKILL: usize = 64 * 1024;
const MAX_LICENSE: usize = 32 * 1024;
/// Directory levels of a packaged file, its top-level directory included
/// (`scripts/a/b/c/x.py` is the deepest allowed).
const MAX_DEPTH: usize = 4;

/// Where a skill package comes from. JSON: `{ "path": "C:\\…" }` or
/// `{ "bytes": { "fileName": "x.zip", "bytes": [80, 75, …] } }`.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
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
    /// `scripts/`, `references/` and `assets/` files, sorted by path.
    pub(crate) files: Vec<PackageFile>,
}

/// One file under `scripts/`, `references/` or `assets/`.
#[derive(Debug)]
pub(crate) struct PackageFile {
    /// Package-relative, `/`-separated.
    pub(crate) path: String,
    pub(crate) kind: SkillFileKind,
    /// Lowercase hex `sha256` of `content`.
    pub(crate) sha256: String,
    pub(crate) content: Vec<u8>,
}

/// Reads and fully validates a package.
pub(crate) fn read(source: &Source) -> Result<Package, KitError> {
    match source {
        Source::Path(path) => read_path(path),
        Source::Bytes { file_name, bytes } if is_zip(Path::new(file_name)) => read_zip(bytes),
        Source::Bytes { bytes, .. } => {
            let mut files = Files::default();
            files.put(
                "SKILL.md",
                Slot::Skill,
                read_capped(bytes.as_slice(), MAX_SKILL)?,
            )?;
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
        let mut files = Files::default();
        walk(path, "", &mut files)?;
        return files.finish();
    }
    if is_zip(path) {
        let bytes = read_capped(File::open(path)?, MAX_ZIP)?;
        return read_zip(&bytes);
    }
    let mut files = Files::default();
    files.put(
        "SKILL.md",
        Slot::Skill,
        read_capped(File::open(path)?, MAX_SKILL)?,
    )?;
    files.finish()
}

/// Reads `dir` as the package directory at `prefix` (`""` = root). Only `scripts/`,
/// `references/` and `assets/` are descended into; links (including Windows junctions)
/// are refused, not followed.
fn walk(dir: &Path, prefix: &str, files: &mut Files) -> Result<(), KitError> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        files.count()?;
        let meta = fs::symlink_metadata(&path)?;
        if is_link(&meta) {
            return Err(KitError::bad_request(&rel, "link"));
        }
        if meta.is_dir() {
            if prefix.is_empty() && file_kind(&rel).is_none() {
                files.unsupported.push(format!("{rel}/"));
            } else {
                check_path(&rel, rel.matches('/').count() + 1)?;
                walk(&path, &rel, files)?;
            }
        } else if let Some(slot) = classify(&rel)?.filter(|_| meta.is_file()) {
            let data = read_capped(File::open(&path)?, files.cap(slot))?;
            files.put(&rel, slot, data)?;
        } else {
            files.unsupported.push(rel);
        }
    }
    Ok(())
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
        match classify(name)? {
            Some(slot) => {
                let entry = archive.by_index(*index).map_err(invalid_zip)?;
                let data =
                    read_capped(entry, files.cap(slot)).map_err(|e| invalid_zip(e.into()))?;
                files.put(name, slot, data)?;
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

/// What a package path holds.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Skill,
    License,
    File(SkillFileKind),
}

impl Slot {
    fn limit(self) -> usize {
        match self {
            Self::Skill => MAX_SKILL,
            Self::License => MAX_LICENSE,
            Self::File(_) => MAX_FILE,
        }
    }
}

/// `None` = not part of the package format (reported as `unsupported_files`). Paths
/// under the three file directories must have valid names and depth.
fn classify(path: &str) -> Result<Option<Slot>, KitError> {
    match path {
        "SKILL.md" => return Ok(Some(Slot::Skill)),
        "LICENSE" | "LICENSE.md" | "LICENSE.txt" => return Ok(Some(Slot::License)),
        _ => {}
    }
    match file_kind(path) {
        Some(kind) if path.contains('/') => {
            check_path(path, path.matches('/').count())?;
            Ok(Some(Slot::File(kind)))
        }
        _ => Ok(None),
    }
}

/// The kind given by the first path segment.
fn file_kind(path: &str) -> Option<SkillFileKind> {
    match path.split('/').next() {
        Some("scripts") => Some(SkillFileKind::Script),
        Some("references") => Some(SkillFileKind::Reference),
        Some("assets") => Some(SkillFileKind::Asset),
        _ => None,
    }
}

/// Every segment matches `[A-Za-z0-9._-]+` without a leading `.`; at most
/// [`MAX_DEPTH`] directory levels.
fn check_path(path: &str, levels: usize) -> Result<(), KitError> {
    let valid = |segment: &str| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !path.split('/').all(valid) {
        return Err(KitError::bad_request(path, "invalid_name"));
    }
    if levels > MAX_DEPTH {
        return Err(KitError::bad_request(path, "too_deep").with_limit(MAX_DEPTH as u64));
    }
    Ok(())
}

#[derive(Default)]
struct Files {
    skill: Option<String>,
    license: Option<String>,
    files: Vec<PackageFile>,
    unsupported: Vec<String>,
    /// Bytes read so far, for [`MAX_TOTAL`].
    total: usize,
    /// Directory entries visited, for [`MAX_ENTRIES`].
    entries: usize,
}

impl Files {
    /// Counts one directory entry against [`MAX_ENTRIES`].
    fn count(&mut self) -> Result<(), KitError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(too_large("entries", MAX_ENTRIES));
        }
        Ok(())
    }

    /// How many bytes to read for `slot`: one past whichever limit is closer, so
    /// [`Files::put`] can tell an oversize file from an exhausted package budget.
    fn cap(&self, slot: Slot) -> usize {
        slot.limit().min(MAX_TOTAL.saturating_sub(self.total))
    }

    fn put(&mut self, name: &str, slot: Slot, bytes: Vec<u8>) -> Result<(), KitError> {
        let limit = slot.limit();
        if bytes.len() > limit {
            return Err(too_large(name, limit));
        }
        self.total += bytes.len();
        if self.total > MAX_TOTAL {
            return Err(too_large("total", MAX_TOTAL));
        }
        let kind = match slot {
            Slot::Skill | Slot::License => {
                let text = text(name, bytes)?;
                let target = if matches!(slot, Slot::Skill) {
                    &mut self.skill
                } else {
                    &mut self.license
                };
                if target.replace(text).is_some() {
                    return Err(KitError::bad_request(name, "duplicate"));
                }
                return Ok(());
            }
            Slot::File(kind) => kind,
        };
        let content = match kind {
            SkillFileKind::Script | SkillFileKind::Reference => text(name, bytes)?.into_bytes(),
            // Assets may be binary; text assets are still checked for credentials.
            SkillFileKind::Asset => match String::from_utf8(bytes) {
                Ok(text) => {
                    check_secret(name, &text)?;
                    text.into_bytes()
                }
                Err(error) => error.into_bytes(),
            },
        };
        self.files.push(PackageFile {
            path: name.to_owned(),
            kind,
            sha256: hex(&Sha256::digest(&content)),
            content,
        });
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
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Package {
            skill: frontmatter::parse(&skill)?,
            license: self.license,
            files: self.files,
        })
    }
}

/// UTF-8 text without anything that looks like a credential.
fn text(name: &str, bytes: Vec<u8>) -> Result<String, KitError> {
    let text = String::from_utf8(bytes).map_err(|_| KitError::bad_request(name, "not_utf8"))?;
    check_secret(name, &text)?;
    Ok(text)
}

/// Reports the line only; the matched text never reaches errors or logs (docs/14 §2.2).
fn check_secret(name: &str, text: &str) -> Result<(), KitError> {
    match mask::find_secret(text) {
        Some((line, _)) => Err(KitError::bad_request(
            &format!("{name}:{line}"),
            "secret_detected",
        )),
        None => Ok(()),
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
