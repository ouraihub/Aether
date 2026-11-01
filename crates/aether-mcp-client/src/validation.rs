use std::path::Path;

use aether_support::{KitError, mask, net, secret};
use url::Url;

use crate::credentials::Secret;
use crate::http;
use crate::{Approval, McpTransport, ServerSecret};

const MAX_COMMAND_CHARS: usize = 1024;
const MAX_ARGS: usize = 64;
const MAX_ARG_CHARS: usize = 4096;

pub(super) struct Columns {
    pub(super) kind: &'static str,
    /// Empty for stdio.
    pub(super) url: String,
    pub(super) command: Option<String>,
    /// JSON array.
    pub(super) args: Option<String>,
    pub(super) cwd: Option<String>,
}

impl Columns {
    pub(super) fn new(transport: &McpTransport, allow_private: bool) -> Result<Self, KitError> {
        match transport {
            McpTransport::Http { url } => {
                check_server_url(url, allow_private)?;
                Ok(Self {
                    kind: "http",
                    url: url.clone(),
                    command: None,
                    args: None,
                    cwd: None,
                })
            }
            McpTransport::Stdio { command, args, cwd } => {
                check_stdio(command, args, cwd.as_deref())?;
                Ok(Self {
                    kind: "stdio",
                    url: String::new(),
                    command: Some(command.clone()),
                    args: Some(serde_json::to_string(args).map_err(KitError::internal)?),
                    cwd: cwd.clone(),
                })
            }
        }
    }
}

pub(super) fn transport_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpTransport> {
    let kind: String = row.get(8)?;
    let url: String = row.get(3)?;
    let command: Option<String> = row.get(9)?;
    let args: Option<String> = row.get(10)?;
    let cwd: Option<String> = row.get(11)?;
    let corrupt = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            format!("server row: {what}").into(),
        )
    };
    match (kind.as_str(), command, args) {
        ("http", None, None) if cwd.is_none() && !url.is_empty() => Ok(McpTransport::Http { url }),
        ("stdio", Some(command), Some(args)) if url.is_empty() => {
            let args = serde_json::from_str(&args).map_err(|_| corrupt("args"))?;
            Ok(McpTransport::Stdio { command, args, cwd })
        }
        _ => Err(corrupt("transport")),
    }
}

/// Seals the new secret; `None` = cleared. Fails before anything is written. http takes
/// headers/query, stdio takes env; the other kind is rejected.
pub(super) fn seal(input: ServerSecret, stdio: bool) -> Result<Option<Vec<u8>>, KitError> {
    let secret = Secret {
        headers: input.headers.unwrap_or_default(),
        query: input.query.unwrap_or_default(),
        env: input.env.unwrap_or_default(),
    };
    let foreign = if stdio {
        !secret.headers.is_empty() || !secret.query.is_empty()
    } else {
        !secret.env.is_empty()
    };
    if foreign {
        return Err(KitError::bad_request("secret", "transport_mismatch"));
    }
    if secret.is_empty() {
        return Ok(None);
    }
    secret.header_map()?;
    secret.check_env()?;
    let plain =
        zeroize::Zeroizing::new(serde_json::to_string(&secret).map_err(KitError::internal)?);
    secret::seal(&plain).map(Some)
}

/// The URL must pass the address policy and carry no credentials (docs/27 §2).
fn check_server_url(url: &str, allow_private: bool) -> Result<(), KitError> {
    let parsed = net::check_url(url, &http::policy(url, allow_private))?;
    let masked = Url::parse(&mask::mask_url(url)).map_err(KitError::internal)?;
    if !parsed.query_pairs().eq(masked.query_pairs()) {
        return Err(KitError::bad_request("url", "credential_query"));
    }
    Ok(())
}

/// docs/27 §2: command non-empty, ≤ 1024 characters, single line; ≤ 64 args of ≤ 4096
/// characters; `cwd` an existing absolute directory. Args are listed unmasked, so a
/// credential-looking one is rejected: it belongs in `secret.env`.
pub(crate) fn check_stdio(
    command: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<(), KitError> {
    if command.trim().is_empty() {
        return Err(KitError::bad_request("command", "empty"));
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(KitError::bad_request("command", "too_long"));
    }
    if command.contains(['\n', '\r', '\0']) {
        return Err(KitError::bad_request("command", "invalid"));
    }
    if args.len() > MAX_ARGS {
        return Err(KitError::bad_request("args", "too_many"));
    }
    if args.iter().any(|arg| arg.chars().count() > MAX_ARG_CHARS) {
        return Err(KitError::bad_request("args", "too_long"));
    }
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err(KitError::bad_request("args", "invalid"));
    }
    let secret_arg = |arg: &String| arg.split('=').any(mask::looks_like_secret);
    if args.iter().any(secret_arg) {
        return Err(KitError::bad_request("args", "credential_arg"));
    }
    if let Some(cwd) = cwd {
        check_cwd(cwd)?;
    }
    Ok(())
}

fn check_cwd(cwd: &str) -> Result<(), KitError> {
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err(KitError::bad_request("cwd", "not_absolute"));
    }
    if !path.is_dir() {
        return Err(KitError::bad_request("cwd", "not_found"));
    }
    Ok(())
}

pub(super) fn validate_slug(slug: &str) -> Result<(), KitError> {
    let valid = (1..=24).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(KitError::bad_request("slug", "invalid"))
    }
}

pub(super) const fn approval_str(approval: Approval) -> &'static str {
    match approval {
        Approval::Auto => "auto",
        Approval::Ask => "ask",
        Approval::Deny => "deny",
    }
}

/// The CHECK constraint admits only the three values; anything else is treated as `ask`.
pub(super) fn parse_approval(value: &str) -> Approval {
    match value {
        "auto" => Approval::Auto,
        "deny" => Approval::Deny,
        _ => Approval::Ask,
    }
}
