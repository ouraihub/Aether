//! Restricted frontmatter parser (docs/14 §2.1). Accepts a flat `key: value`
//! subset of YAML and rejects everything else, so no YAML feature (anchors,
//! tags, block scalars, documents) can make the stored skill differ from what
//! the user previewed.

use std::collections::BTreeMap;

use aether_support::KitError;

const MAX_FRONTMATTER: usize = 4 * 1024;
const MAX_KEY: usize = 32;
const MAX_DESCRIPTION: usize = 500;
const MAX_VERSION: usize = 32;
/// Same as the `SKILL.md` cap of docs/14 §2.2, so an edited skill still fits
/// in a package.

/// A parsed `SKILL.md`.
#[derive(Debug)]
pub(crate) struct SkillFile {
    pub(crate) name: String,
    pub(crate) description: String,
    /// `None` = unversioned; never invented.
    pub(crate) version: Option<String>,
    /// Valid keys we do not interpret, shown as-is.
    pub(crate) extra: BTreeMap<String, String>,
    /// Body after the closing `---`, CRLF normalized, otherwise verbatim
    /// (docs/14 §7.2).
    pub(crate) body: String,
}

/// Parses `SKILL.md` text. Syntax outside the subset fails with
/// `bad_request{reason:"frontmatter_unsupported", field:"<line>"}`.
pub(crate) fn parse(text: &str) -> Result<SkillFile, KitError> {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n").ok_or_else(|| unsupported(1))?;

    let mut fields = BTreeMap::new();
    let mut consumed = 0;
    let mut line_no = 1;
    let mut lines = rest.split_inclusive('\n');
    let body = loop {
        line_no += 1;
        let raw = lines.next().ok_or_else(|| unsupported(line_no))?;
        consumed += raw.len();
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        if line == "---" {
            break &rest[consumed..];
        }
        if consumed > MAX_FRONTMATTER {
            return Err(unsupported(line_no).with_limit(MAX_FRONTMATTER as u64));
        }
        let (key, value) = parse_line(line)
            .ok_or_else(|| unsupported(line_no))?;
        if fields.insert(key.to_owned(), value).is_some() {
            return Err(unsupported(line_no));
        }
    };
    // A second `---` right after the closing one starts another YAML document.
    if body.lines().next() == Some("---") {
        return Err(unsupported(line_no + 1));
    }

    let name = fields
        .remove("name")
        .ok_or_else(|| KitError::bad_request("name", "missing"))?;
    let description = fields
        .remove("description")
        .ok_or_else(|| KitError::bad_request("description", "missing"))?;
    let version = fields.remove("version");
    validate(&name, &description, version.as_deref(), body)?;
    Ok(SkillFile {
        name,
        description,
        version,
        extra: fields,
        body: body.to_owned(),
    })
}

/// Field rules shared by packages, hand-written skills and built-ins
/// (docs/14 §2.1).
pub(crate) fn validate(
    name: &str,
    description: &str,
    version: Option<&str>,
    body: &str,
) -> Result<(), KitError> {
    if !is_valid_name(name) {
        return Err(KitError::bad_request("name", "invalid"));
    }
    if !(1..=MAX_DESCRIPTION).contains(&description.chars().count()) {
        return Err(
            KitError::bad_request("description", "invalid")
                .with_limit(MAX_DESCRIPTION as u64),
        );
    }
    if version.is_some_and(|v| v.chars().count() > MAX_VERSION) {
        return Err(
            KitError::bad_request("version", "invalid")
                .with_limit(MAX_VERSION as u64),
        );
    }
    if body.trim().is_empty() {
        return Err(KitError::bad_request("body", "empty"));
    }
    if body.len() > MAX_BODY {
        return Err(
            KitError::bad_request("body", "too_large")
                .with_limit(MAX_BODY as u64),
        );
    }
    Ok(())
}

fn unsupported(line: usize) -> KitError {
    KitError::bad_request(&line.to_string(), "frontmatter_unsupported")
}

/// `key: value` with `key` matching `^[a-z][a-z0-9_-]{0,31}$`.
/// Indented lines, list items and `key:` openers of nested maps fail the key
/// or separator check.
fn parse_line(line: &str) -> Option<(&str, String)> {
    let (key, value) = line.split_once(": ")?;
    let valid_key = key.len() <= MAX_KEY
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || c == '_'
                    || c == '-'
            });
    if !valid_key {
        return None;
    }
    scalar(value.trim()).map(|value| (key, value))
}

fn scalar(value: &str) -> Option<String> {
    if let Some(inner) = value.strip_prefix('"') {
        // Only `\"` and `\\` escapes; anything after the closing quote is
        // refused.
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return chars.as_str().is_empty().then_some(out),
                '\\' => match chars.next()? {
                    c @ ('"' | '\\') => out.push(c),
                    _ => return None,
                },
                c => out.push(c),
            }
        }
        return None;
    }
    if let Some(inner) = value.strip_prefix('\'') {
        let inner = inner.strip_suffix('\'')?;
        return (!inner.contains('\'')).then(|| inner.to_owned());
    }
    // (flow collections, anchors, aliases, tags, block scalars, lists,
    // comments).
    let indicator = value.starts_with([
        '[', ']', '{', '}', ',', '#', '&', '*', '!', '|', '>', '%', '@', '`',
    ]) || value == "-"
        || value.starts_with("- ")
        || value.starts_with("? ");
    let ok = !value.is_empty()
        && !indicator
        && !value.contains(" #")
        && !value.contains(": ");
    ok.then(|| value.to_owned())
}

fn is_valid_name(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with(|c: char| {
            c.is_ascii_lowercase() || c.is_ascii_digit()
        })
        && name
            .chars()
            .all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
            })
}
