//! Read-only marketplace discovery and immutable GitHub skill acquisition.
//!
//! Downloads stay in memory and never execute package content. Call
//! [`crate::Store::preview`] and require digest confirmation before installing
//! the returned [`crate::Source`].
//!
//! Uses bounded HTTPS reads only; no Git, package manager or downloaded code
//! is executed.

use std::collections::HashSet;
use std::io::{Cursor, Write};
use std::time::Duration;

use aether_support::KitError;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

const REGISTRY: &str = "https://api.claude-plugins.dev";
const GITHUB_API: &str = "https://api.github.com";
const MAX_FILE: usize = 1024 * 1024;
const MAX_TOTAL: usize = 16 * 1024 * 1024;
const MAX_ZIP: usize = 5 * 1024 * 1024;
const MAX_ENTRIES: usize = 200;
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const MAX_REPO_ENTRIES: usize = 100_000;

/// A marketplace result with an explicit GitHub skill location.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkill {
    /// Registry name, or directory name for GitHub discovery.
    pub name: String,
    /// Registry description; not trusted package metadata.
    pub description: String,
    /// GitHub location accepted by [`prepare`].
    pub url: String,
    /// Registry name or pinned GitHub commit URL.
    pub origin: String,
}

/// Prepared package data; importing still requires Aether preview and
/// confirmation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedSkill {
    /// JSON-compatible Aether source, containing an in-memory ZIP.
    pub source: crate::Source,
    /// Immutable commit location, or local provenance for system imports.
    pub origin: String,
    /// Paths in the selected directory excluded by the package whitelist.
}

#[derive(Deserialize)]
struct SearchResponse {
    skills: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct SearchItem {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "sourceUrl")]
    source_url: String,
    metadata: SearchMetadata,
}
#[derive(Deserialize)]
struct SearchMetadata {
    #[serde(rename = "directoryPath")]
    directory_path: String,
    #[serde(rename = "repoOwner")]
    owner: String,
    #[serde(rename = "repoName")]
    repo: String,
}
#[derive(Deserialize)]
struct RefObject {
    object: GitObject,
}
#[derive(Debug, Deserialize)]
struct GitObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct Commit {
    sha: String,
}
#[derive(Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
    truncated: bool,
}
#[derive(Debug, Clone, Deserialize)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Repository,
    Directory,
    Descriptor,
}
#[derive(Debug, Clone)]
struct GithubLocation {
    owner: String,
    repo: String,
    tail: Vec<String>,
    namespace: Option<String>,
    target: Target,
}

fn invalid(field: &str, reason: &str) -> KitError {
    KitError::bad_request(field, reason)
}

fn network(error: reqwest::Error) -> KitError {
    aether_support::net::transport(error)
}

fn client() -> Result<Client, KitError> {
    aether_support::net::client(
        aether_support::net::AddressPolicy::default(),
        Duration::from_secs(15),
    )
}

fn segment(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part.len() <= 255
        && part
            .bytes()
            .all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')
            })
}

fn repo_path(parts: &[String]) -> bool {
    parts.len() <= 32 && parts.iter().all(|part| segment(part))
}

fn reserved(part: &str) -> bool {
    let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && matches!(base.as_bytes()[3], b'1'..=b'9'))
}

fn package_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    parts.len() <= 5 // Aether permits four directory levels, plus the filename.
        && parts.iter().all(|part| {
            segment(part)
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !reserved(part)
        })
}

fn parse_github_url(input: &str) -> Result<GithubLocation, KitError> {
    // Check before URL parsing: the URL parser otherwise silently removes dot
    // segments.
    if input.len() > 4096
        || input.contains(['\\', '%'])
        || input.bytes().any(|b| b.is_ascii_control())
    {
        return Err(invalid("url", "malformed"));
    }
    let raw = input
        .strip_prefix("https://")
        .ok_or_else(|| invalid("url", "scheme"))?;
    let (_, raw_path) = raw
        .split_once('/')
        .ok_or_else(|| invalid("url", "malformed"))?;
    let raw_path = raw_path.strip_suffix('/').unwrap_or(raw_path);
    if raw_path
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(invalid("url", "path"));
    }
    let url = Url::parse(input).map_err(|_| invalid("url", "malformed"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
    {
        return Err(invalid("url", "forbidden"));
    }
    let host = url.host_str().ok_or_else(|| invalid("url", "host"))?;
    if host != "github.com" && host != "raw.githubusercontent.com" {
        return Err(invalid("url", "host"));
    }
    let parts: Vec<String> = raw_path.split('/').map(str::to_owned).collect();
    if parts.len() < 2 || !repo_path(&parts) {
        return Err(invalid("url", "path"));
    }
    let owner = parts[0].clone();
    let repo = parts[1]
        .strip_suffix(".git")
        .unwrap_or(&parts[1])
        .to_owned();
    if !segment(&repo) {
        return Err(invalid("url", "path"));
    }
    if host == "github.com" && parts.len() == 2 {
        return Ok(GithubLocation {
            owner,
            repo,
            tail: Vec::new(),
            namespace: None,
            target: Target::Repository,
        });
    }
    let (mut tail, target) = if host == "github.com" {
        let target = match parts.get(2).map(String::as_str) {
            Some("tree") => Target::Directory,
            Some("blob") => Target::Descriptor,
            _ => return Err(invalid("url", "unsupported")),
        };
        (parts[3..].to_vec(), target)
    } else {
        (parts[2..].to_vec(), Target::Descriptor)
    };
    if target == Target::Descriptor
        && tail.pop().as_deref() != Some("SKILL.md")
    {
        return Err(invalid("url", "descriptor"));
    }
    let namespace = if tail.first().map(String::as_str) == Some("refs") {
        let ns = tail
            .get(1)
            .filter(|ns| matches!(ns.as_str(), "heads" | "tags"))
            .ok_or_else(|| invalid("url", "ref"))?
            .clone();
        tail.drain(..2);
        Some(ns)
    } else {
        None
    };
    if tail.is_empty() {
        return Err(invalid("url", "ref"));
    }
    Ok(GithubLocation {
        owner,
        repo,
        tail,
        namespace,
        target,
    })
}

fn api_url(parts: &[&str]) -> Result<Url, KitError> {
    let mut url =
        Url::parse(GITHUB_API).map_err(|_| invalid("url", "malformed"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| invalid("url", "malformed"))?;
        for part in parts {
            segments.push(part);
        }
    }
    Ok(url)
}

async fn response_bytes(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, KitError> {
    let status = response.status();
    if !status.is_success() {
        // Do not echo arbitrary upstream bodies or URLs. Status and rate-limit
        // reason remain visible.
        let rate_limit = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .is_some_and(|v| v == "0"));
        let error = KitError::upstream(status.as_u16(), "", &[]);
        return Err(if rate_limit {
            error.with_reason("rate_limited")
        } else {
            error
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(invalid("response", "too_large").with_limit(limit as u64));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(
                invalid("response", "too_large").with_limit(limit as u64),
            );
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, KitError> {
    let bytes = response_bytes(response, MAX_RESPONSE).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| invalid("response", "invalid_metadata"))
}

fn sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn ref_object(
    http: &Client,
    location: &GithubLocation,
    namespace: &str,
    parts: &[String],
) -> Result<Option<GitObject>, KitError> {
    let mut url = api_url(&[
        "repos",
        &location.owner,
        &location.repo,
        "git",
        "ref",
        namespace,
    ])?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| invalid("url", "malformed"))?;
        for part in parts {
            segments.push(part);
        }
    }
    let response = http
        .get(url)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(network)?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let value: RefObject = json(response).await?;
    Ok(Some(value.object))
}

fn choose_ref(
    branch: Option<GitObject>,
    tag: Option<GitObject>,
) -> Result<Option<GitObject>, KitError> {
    if branch.is_some() && tag.is_some() {
        return Err(invalid("url", "ambiguous_ref"));
    }
    Ok(branch.or(tag))
}

async fn resolve_commit(
    http: &Client,
    location: &GithubLocation,
) -> Result<(String, Vec<String>), KitError> {
    if location.target == Target::Repository {
        let url = api_url(&[
            "repos",
            &location.owner,
            &location.repo,
            "commits",
            "HEAD",
        ])?;
        let value: Commit = json(
            http.get(url)
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .map_err(network)?,
        )
        .await?;
        if !sha(&value.sha) {
            return Err(invalid("response", "commit"));
        }
        return Ok((value.sha, Vec::new()));
    }
    if location.namespace.is_none() && sha(&location.tail[0]) {
        return Ok((
            location.tail[0].to_ascii_lowercase(),
            location.tail[1..].to_vec(),
        ));
    }
    for count in (1..=location.tail.len()).rev() {
        let parts = &location.tail[..count];
        let branch = if location.namespace.as_deref() != Some("tags") {
            ref_object(http, location, "heads", parts).await?
        } else {
            None
        };
        let tag = if location.namespace.as_deref() != Some("heads") {
            ref_object(http, location, "tags", parts).await?
        } else {
            None
        };
        if let Some(mut object) = choose_ref(branch, tag)? {
            // Annotated tags point to another tag object, not necessarily
            // directly to a commit.
            for _ in 0..5 {
                if !sha(&object.sha) {
                    return Err(invalid("response", "commit"));
                }
                if object.kind == "commit" {
                    return Ok((object.sha, location.tail[count..].to_vec()));
                }
                if object.kind != "tag" {
                    return Err(invalid("url", "ref_not_commit"));
                }
                let url = api_url(&[
                    "repos",
                    &location.owner,
                    &location.repo,
                    "git",
                    "tags",
                    &object.sha,
                ])?;
                let value: RefObject = json(
                    http.get(url)
                        .timeout(Duration::from_secs(60))
                        .send()
                        .await
                        .map_err(network)?,
                )
                .await?;
                object = value.object;
            }
            return Err(invalid("url", "tag_depth"));
        }
    }
    Err(invalid("url", "ref_not_found"))
}


#[path = "sources_remote.rs"]
mod remote;
use remote::{
    candidates, choose_target, file_limit, origin, prefix, selected,
    zip_package,
};
pub use remote::{prepare, search};
#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
