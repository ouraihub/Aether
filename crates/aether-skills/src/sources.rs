//! Read-only marketplace discovery and immutable GitHub skill acquisition.
//!
//! Downloads stay in memory and never execute package content. Call [`crate::Store::preview`]
//! and require digest confirmation before installing the returned [`crate::Source`].
//!
//! Uses bounded HTTPS reads only; no Git, package manager or downloaded code is executed.

use aether_support::KitError;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::time::Duration;

mod github;
mod package;

#[cfg(test)]
use github::{GitObject, TreeEntry, choose_ref, package_path, sha};
use github::{
    Target, client, invalid, json, network, parse_github_url, repo_path, resolve_commit, tree,
};
use package::{
    candidates, choose_target, download, file_limit, origin, prefix, selected, zip_package,
};

const REGISTRY: &str = "https://api.claude-plugins.dev";
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

/// Prepared package data; importing still requires Aether preview and confirmation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedSkill {
    /// JSON-compatible Aether source, containing an in-memory ZIP.
    pub source: crate::Source,
    /// Immutable commit location, or local provenance for system imports.
    pub origin: String,
    /// Paths in the selected directory excluded by the package whitelist.
    pub ignored: Vec<String>,
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

/// Searches the real Claude Plugins registry, or discovers skills at a GitHub URL.
///
/// # Errors
/// Returns invalid input, bounded-response, upstream/rate-limit, or network errors.
/// A GitHub repository URL returns all valid candidates, never an arbitrary first skill.
pub async fn search(query: &str) -> Result<Vec<RemoteSkill>, KitError> {
    let query = query.trim();
    if query.len() > 4096 {
        return Err(invalid("query", "too_large"));
    }
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query.contains("://") {
        return discover(query).await;
    }
    let http = client()?;
    let mut url = Url::parse(&format!("{REGISTRY}/api/skills/search"))
        .map_err(|_| invalid("url", "malformed"))?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("limit", "20");
    let value: SearchResponse = json(
        http.get(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(network)?,
    )
    .await?;
    if value.skills.len() > MAX_ENTRIES {
        return Err(invalid("response", "too_many_results"));
    }
    Ok(value
        .skills
        .into_iter()
        .filter_map(|item| {
            let location = parse_github_url(&item.source_url).ok()?;
            let directory: Vec<String> = if item.metadata.directory_path.is_empty() {
                Vec::new()
            } else {
                item.metadata
                    .directory_path
                    .split('/')
                    .map(str::to_owned)
                    .collect()
            };
            if location.owner != item.metadata.owner
                || location.repo != item.metadata.repo
                || !repo_path(&directory)
                || location.target == Target::Repository
            {
                return None;
            }
            // Registry directoryPath is explicit; require the source URL to name that exact directory.
            if location.tail.len() <= directory.len()
                || location.tail[location.tail.len() - directory.len()..] != directory
            {
                return None;
            }
            Some(RemoteSkill {
                name: item.name,
                description: item.description,
                url: item.source_url,
                origin: "claude-plugins.dev".into(),
            })
        })
        .collect())
}

async fn discover(input: &str) -> Result<Vec<RemoteSkill>, KitError> {
    let location = parse_github_url(input)?;
    let http = client()?;
    let (commit, path) = resolve_commit(&http, &location).await?;
    let entries = tree(&http, &location, &commit).await?;
    let dirs = if location.target == Target::Repository {
        candidates(&entries)?
    } else {
        selected(&entries, &path)?;
        vec![path]
    };
    if dirs.is_empty()
        && entries
            .iter()
            .any(|entry| entry.path.rsplit('/').next() == Some("SKILL.md"))
    {
        return Err(invalid("repository", "no_compatible_skills"));
    }
    Ok(dirs
        .into_iter()
        .map(|dir| {
            let url = origin(&location, &commit, &dir);
            RemoteSkill {
                name: dir.last().cloned().unwrap_or_else(|| location.repo.clone()),
                description: "GitHub skill".into(),
                origin: url.clone(),
                url,
            }
        })
        .collect())
}

/// Resolves a GitHub URL to one immutable commit and downloads only its skill whitelist.
///
/// # Errors
/// Rejects ambiguous refs, multiple repository-root candidates, unsafe paths, links,
/// truncated trees, excessive files/bytes, and upstream/network failures. A directory
/// must contain its own `SKILL.md`; callers cannot select arbitrary repository files.
pub async fn prepare(url: &str) -> Result<PreparedSkill, KitError> {
    let location = parse_github_url(url)?;
    let http = client()?;
    let (commit, path) = resolve_commit(&http, &location).await?;
    let entries = tree(&http, &location, &commit).await?;
    let target = choose_target(&entries, &location, path)?;
    let (selected_entries, ignored) = selected(&entries, &target)?;
    let mut total = 0usize;
    let mut files = Vec::with_capacity(selected_entries.len());
    let prefix = prefix(&target);
    for entry in selected_entries {
        let relative = entry
            .path
            .strip_prefix(&prefix)
            .ok_or_else(|| invalid("path", "unsafe"))?;
        let bytes = download(&http, &location, &commit, &entry.path, file_limit(relative)).await?;
        total += bytes.len();
        if total > MAX_TOTAL {
            return Err(invalid("skill", "too_large").with_limit(MAX_TOTAL as u64));
        }
        files.push((relative.to_owned(), bytes));
    }
    let bytes = zip_package(&files)?;
    let name = target.last().unwrap_or(&location.repo);
    Ok(PreparedSkill {
        source: crate::Source::Bytes {
            file_name: format!("{name}.zip"),
            bytes,
        },
        origin: origin(&location, &commit, &target),
        ignored,
    })
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
