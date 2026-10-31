//! Per-application data root, decided once at startup (decision D1, docs/10 §1.1).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::KitError;

const PROBE_FILE: &str = ".aether-write-probe";

/// `<exe dir>/data` when it is writable (portable install), otherwise the user's
/// data directory: `%LOCALAPPDATA%\<slug>`, `~/Library/Application Support/<slug>`,
/// or `$XDG_DATA_HOME/<slug>`. Call once at startup; never re-resolved at run time.
///
/// # Errors
/// `bad_request` for an invalid slug; `unsupported` when no user data directory can be
/// determined; `internal` when the chosen directory cannot be created.
pub fn data_root(app_slug: &str) -> Result<PathBuf, KitError> {
    validate_slug(app_slug)?;
    let exe = std::env::current_exe().map_err(KitError::internal)?;
    let portable = exe.parent().map(|dir| dir.join("data"));
    let root = resolve(portable.as_deref(), app_slug, user_data_dir)?;
    tracing::info!(
        root = %root.display(),
        portable = portable.as_deref() == Some(root.as_path()),
        "aether.data_root"
    );
    Ok(root)
}

fn resolve(
    portable: Option<&Path>,
    app_slug: &str,
    user_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, KitError> {
    if let Some(dir) = portable
        && is_writable(dir)
    {
        return Ok(dir.to_path_buf());
    }
    let root = user_dir()
        .ok_or_else(|| KitError::unsupported("no_user_data_dir"))?
        .join(app_slug);
    fs::create_dir_all(&root)?;
    Ok(root)
}

/// Creates `dir` if needed and proves it accepts writes.
fn is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(PROBE_FILE);
    let writable = fs::write(&probe, b"").is_ok();
    // An antivirus may hold the file briefly; a leftover empty probe is harmless.
    let _ = fs::remove_file(&probe);
    writable
}

fn validate_slug(slug: &str) -> Result<(), KitError> {
    let valid = (1..=32).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(KitError::bad_request("appSlug", "invalid_slug"))
    }
}

fn user_data_dir() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(target_os = "windows") {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|home| home.join(".local/share")))
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use super::*;

    #[test]
    fn prefers_a_writable_portable_dir() {
        let temp = tempfile::tempdir().unwrap();
        let portable = temp.path().join("app 应用").join("data");
        let unused_user_dir = temp.path().join("must-not-be-created");
        let root = resolve(Some(&portable), "demo", || Some(unused_user_dir.clone())).unwrap();
        assert!(
            !unused_user_dir.exists(),
            "user dir is not consulted when portable works"
        );
        assert_eq!(root, portable);
        assert!(!portable.join(PROBE_FILE).exists(), "probe file is removed");
    }

    #[test]
    fn falls_back_to_user_dir_when_portable_is_not_writable() {
        let temp = tempfile::tempdir().unwrap();
        // A file where the directory should be makes create_dir_all fail.
        let blocker = temp.path().join("data");
        fs::write(&blocker, b"").unwrap();
        let user = temp.path().join("user");
        let root = resolve(Some(&blocker), "demo", || Some(user.clone())).unwrap();
        assert_eq!(root, user.join("demo"));
        assert!(root.is_dir());
    }

    #[test]
    fn rejects_bad_slugs_and_missing_user_dir() {
        for slug in ["", "Demo", "-demo", "demo-", "de mo", &"a".repeat(33)] {
            assert_eq!(
                validate_slug(slug).unwrap_err().code(),
                crate::KitErrorCode::BadRequest,
                "{slug}"
            );
        }
        let error = resolve(None, "demo", || None).unwrap_err();
        assert_eq!(error.code(), crate::KitErrorCode::Unsupported);
    }
}
