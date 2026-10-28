//! Pure content identity for validated skill text and sorted packaged-file hashes.

use sha2::{Digest, Sha256};

/// Borrowed fields that participate in a skill's identity (docs/14 §3.1).
/// Uninterpreted frontmatter and repository metadata are deliberately excluded.
#[derive(Clone, Copy)]
pub struct ContentIdentity<'a> {
    /// Validated skill slug; encoded verbatim without normalization.
    pub name: &'a str,
    /// Validated display description; encoded as its original UTF-8 bytes.
    pub description: &'a str,
    /// Validated optional version; `None` and an empty string have distinct identities.
    pub version: Option<&'a str>,
    /// Validated skill body; parsing-boundary normalization must already be complete.
    pub body: &'a str,
    /// Validated optional license; presence, even empty, enables the package suffix.
    pub license: Option<&'a str>,
}

/// SHA-256 identity as 64 lowercase hexadecimal digits, not a hash of ZIP bytes.
///
/// # Preconditions
/// Text fields, paths and file hashes must already be validated. `files` must
/// contain unique paths in stable ascending path order and validated lowercase
/// SHA-256 hashes. This function neither validates nor normalizes its inputs;
/// callers remain responsible for package validation.
///
/// Each optional field is encoded as `0` for `None`, or `1`, its UTF-8 byte
/// length as a little-endian u64, and its bytes for `Some`. Name, description,
/// version and body come first. Only when a license is present (even empty) or
/// files exist, the `package` field, license and each path/hash pair follow.
/// No file count or additional separator is written.
pub fn digest<'a>(
    content: ContentIdentity<'_>,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let mut hasher = Sha256::new();
    let mut field = |value: Option<&str>| match value {
        Some(text) => {
            hasher.update([1]);
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        None => hasher.update([0]),
    };
    for value in [
        Some(content.name),
        Some(content.description),
        content.version,
        Some(content.body),
    ] {
        field(value);
    }
    let mut files = files.into_iter().peekable();
    if content.license.is_some() || files.peek().is_some() {
        field(Some("package"));
        field(content.license);
        for (path, sha256) in files {
            field(Some(path));
            field(Some(sha256));
        }
    }
    hex(&hasher.finalize())
}

/// Lowercase hex shared with package file hashing.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}

#[cfg(test)]
mod tests;
