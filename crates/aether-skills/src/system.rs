//! Discovery and in-memory packaging of skills installed by local agent tools.

use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::path::Path;

use aether_support::KitError;

use serde::Serialize;
use zip::write::SimpleFileOptions;

use crate::Source;
use crate::sources::PreparedSkill;
mod discovery;
pub use discovery::{discover, discover_directory, prepare};

const MAX_FILE: usize = 1024 * 1024;
const MAX_SKILL: usize = 64 * 1024;
const MAX_LICENSE: usize = 32 * 1024;
const MAX_TOTAL: usize = 16 * 1024 * 1024;
const MAX_ZIP: usize = 5 * 1024 * 1024;
const MAX_ENTRIES: usize = 200;
const MAX_DEPTH: usize = 4;
const MAX_DISCOVERED: usize = 200;
const MAX_VISITED: usize = 4000;

/// A discovered local package; selection does not install or trust its contents.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemSkill {
    /// Absolute directory path, revalidated before packaging.
    pub path: String,
    /// Directory name for selection; authoritative metadata is read at preview.
    pub name: String,
    /// Known CLI name or user-selected directory label.
    pub origin: String,
}

/// Packages a directory in memory. Callers accepting user-selected paths must first
/// authorize the parent directory with [`discover_directory`].
///
/// # Errors
/// Rejects links, nested skills, invalid names and package size limits.
pub fn prepare_directory(path: &Path) -> Result<PreparedSkill, KitError> {
    let path = discovery::checked_input_path(path)?;
    let meta = fs::symlink_metadata(&path).map_err(KitError::internal)?;
    if !meta.is_dir() {
        return Err(KitError::bad_request("path", "not_directory"));
    }
    let mut files = Vec::new();
    let mut ignored = Vec::new();
    let mut entries = 0usize;
    let mut total = 0usize;
    package_dir(
        &path,
        "",
        &mut files,
        &mut ignored,
        &mut entries,
        &mut total,
    )?;
    if !files.iter().any(|(name, _)| name == "SKILL.md") {
        return Err(KitError::bad_request("SKILL.md", "missing"));
    }
    let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for (name, bytes) in &files {
        zip.start_file(name, SimpleFileOptions::default())
            .map_err(|error| KitError::internal(io::Error::other(error.to_string())))?;
        zip.write_all(bytes).map_err(KitError::internal)?;
    }
    let bytes = zip
        .finish()
        .map_err(|error| KitError::internal(io::Error::other(error.to_string())))?
        .into_inner();
    if bytes.len() > MAX_ZIP {
        return Err(too_large("zip", MAX_ZIP));
    }
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("skill");
    Ok(PreparedSkill {
        source: Source::Bytes {
            file_name: format!("{name}.zip"),
            bytes,
        },
        origin: "Local folder".to_owned(),
        ignored,
    })
}

fn package_dir(
    path: &Path,
    prefix: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    ignored: &mut Vec<String>,
    entries: &mut usize,
    total: &mut usize,
) -> Result<(), KitError> {
    let mut items = Vec::new();
    for item in fs::read_dir(path).map_err(KitError::internal)? {
        if items.len() + *entries >= MAX_ENTRIES {
            return Err(too_large("entries", MAX_ENTRIES));
        }
        items.push(item.map_err(KitError::internal)?);
    }
    items.sort_by_key(|entry| entry.file_name());
    for item in items {
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return Err(too_large("entries", MAX_ENTRIES));
        }
        let child = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(&child).map_err(KitError::internal)?;
        if is_link(&meta) {
            return Err(KitError::bad_request(&relative, "link"));
        }
        if meta.is_dir() {
            if prefix.is_empty() && !matches!(name.as_str(), "scripts" | "references" | "assets") {
                ignored.push(format!("{name}/"));
            } else if prefix.is_empty() || relative.split('/').count() <= MAX_DEPTH {
                package_dir(&child, &relative, files, ignored, entries, total)?;
            } else {
                return Err(KitError::bad_request(&relative, "too_deep"));
            }
            continue;
        }
        if !meta.is_file() {
            ignored.push(relative);
            continue;
        }
        if name.eq_ignore_ascii_case("SKILL.md") && !prefix.is_empty() {
            return Err(KitError::bad_request(&relative, "nested_skill"));
        }
        let allowed = prefix.is_empty() && name == "SKILL.md"
            || prefix.is_empty()
                && matches!(name.as_str(), "LICENSE" | "LICENSE.md" | "LICENSE.txt")
            || matches!(
                prefix.split('/').next(),
                Some("scripts" | "references" | "assets")
            );
        if !allowed {
            ignored.push(relative);
            continue;
        }
        let limit = if relative == "SKILL.md" {
            MAX_SKILL
        } else if prefix.is_empty() {
            MAX_LICENSE
        } else {
            MAX_FILE
        };
        let mut data = Vec::new();
        File::open(&child)
            .map_err(KitError::internal)?
            .take((limit + 1) as u64)
            .read_to_end(&mut data)
            .map_err(KitError::internal)?;
        if data.len() > limit {
            return Err(too_large(&relative, limit));
        }
        *total = total
            .checked_add(data.len())
            .ok_or_else(|| too_large("total", MAX_TOTAL))?;
        if *total > MAX_TOTAL {
            return Err(too_large("total", MAX_TOTAL));
        }
        files.push((relative, data));
    }
    Ok(())
}

pub(super) fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

pub(super) fn too_large(field: &str, limit: usize) -> KitError {
    KitError::bad_request(field, "too_large").with_limit(limit as u64)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "test failures report invalid fixtures")]
    use super::*;

    #[test]
    fn selected_root_discovers_skills_and_packages_only_allowed_files() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let root = root.path().canonicalize().unwrap();
        #[cfg(not(windows))]
        let root = root.as_path();
        #[cfg(windows)]
        let root = root.path();
        let skill = root.join("example");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: example\ndescription: Test skill\n---\nRead the reference.",
        )
        .unwrap();
        fs::write(skill.join("references/context.md"), "Context").unwrap();
        fs::write(skill.join("README.md"), "Not installed").unwrap();
        let found = discover_directory(root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            discover_directory(&skill).unwrap(),
            vec![SystemSkill {
                path: skill.to_string_lossy().into_owned(),
                name: "example".into(),
                origin: "Selected folder".into()
            }]
        );
        let prepared = prepare_directory(&skill).unwrap();
        assert_eq!(prepared.ignored, vec!["README.md"]);
        let store_root = tempfile::tempdir().unwrap();
        let store = crate::Store::open(store_root.path(), &[]).unwrap();
        let preview = store.preview(&prepared.source).unwrap();
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].path, "references/context.md");
        assert!(store.list().unwrap().is_empty());
        let installed = store
            .install(&prepared.source, &preview.digest, false)
            .unwrap();
        assert_eq!(
            store
                .file(
                    &installed[0].id,
                    installed[0].revision,
                    "references/context.md"
                )
                .unwrap(),
            b"Context"
        );
        assert_eq!(
            fs::read_to_string(skill.join("README.md")).unwrap(),
            "Not installed"
        );
        #[cfg(unix)]
        {
            let link = root.join("linked");
            std::os::unix::fs::symlink(&skill, &link).unwrap();
            assert!(discover_directory(&link).is_err());
            assert!(prepare_directory(&link).is_err());
        }
        fs::write(skill.join("references/SKILL.md"), "nested").unwrap();
        assert!(prepare_directory(&skill).is_err());
    }

    #[test]
    fn scan_and_package_limits_reject_without_following_links() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..=MAX_VISITED {
            fs::write(root.path().join(index.to_string()), "").unwrap();
        }
        assert!(discover_directory(root.path()).is_err());
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("SKILL.md"), vec![b'x'; MAX_SKILL + 1]).unwrap();
        assert!(prepare_directory(other.path()).is_err());
        assert!(prepare_directory(Path::new("relative")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn ancestor_junction_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("skill");
        fs::create_dir(&target).unwrap();
        fs::write(
            target.join("SKILL.md"),
            "---\nname: test\ndescription: Test\n---\nBody",
        )
        .unwrap();
        let link = root.path().join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(prepare_directory(&link.join("skill")).is_err());
        fs::remove_dir(link).unwrap();
    }
}
