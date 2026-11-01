//! Lexical path containment for authorization roots; no command or script classification.

use std::path::{Component, Path, PathBuf};

/// Whether `path` lies inside one of `roots`, after resolving `.` and `..` lexically only.
/// Relative paths and roots never match. Symbolic links are not resolved: a host that needs
/// that canonicalizes both sides where it does I/O, before the synchronous `authorize`.
#[must_use]
pub fn within(path: &Path, roots: &[PathBuf]) -> bool {
    let Some(path) = normalize(path) else {
        return false;
    };
    roots.iter().filter_map(|root| normalize(root)).any(|root| {
        let mut inside = path.components();
        root.components()
            .all(|part| inside.next().is_some_and(|other| same(part, other)))
    })
}

fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => out.push(part),
            Component::CurDir => {}
            // `..` at the root stays at the root, as the OS resolves it.
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                }
            }
        }
    }
    Some(out)
}

/// Windows paths compare without case.
fn same(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}
