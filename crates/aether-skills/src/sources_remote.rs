use super::*;

async fn tree(
    http: &Client,
    location: &GithubLocation,
    commit: &str,
) -> Result<Vec<TreeEntry>, KitError> {
    let mut url = api_url(&[
        "repos",
        &location.owner,
        &location.repo,
        "git",
        "trees",
        commit,
    ])?;
    url.query_pairs_mut().append_pair("recursive", "1");
    let value: TreeResponse = json(
        http.get(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(network)?,
    )
    .await?;
    if value.truncated {
        return Err(invalid("repository", "tree_truncated"));
    }
    if value.tree.len() > MAX_REPO_ENTRIES {
        return Err(invalid("repository", "too_many_files"));
    }
    Ok(value.tree)
}

fn prefix(target: &[String]) -> String {
    if target.is_empty() {
        String::new()
    } else {
        format!("{}/", target.join("/"))
    }
}

fn whitelist(path: &str) -> bool {
    matches!(
        path,
        "SKILL.md"
            | "LICENSE"
            | "LICENSE.md"
            | "LICENSE.txt"
            | "scripts"
            | "references"
            | "assets"
    ) || path.starts_with("scripts/")
        || path.starts_with("references/")
        || path.starts_with("assets/")
}

fn selected<'a>(
    entries: &'a [TreeEntry],
    target: &[String],
) -> Result<(Vec<&'a TreeEntry>, Vec<String>), KitError> {
    if !repo_path(target) {
        return Err(invalid("path", "unsafe"));
    }
    for count in 1..=target.len() {
        let ancestor = target[..count].join("/");
        if entries
            .iter()
            .any(|e| {
                e.path.eq_ignore_ascii_case(&ancestor) && e.path != ancestor
            })
        {
            return Err(invalid("path", "duplicate"));
        }
        if !entries.iter().any(|e| {
            e.path == ancestor && e.kind == "tree" && e.mode == "040000"
        }) {
            return Err(invalid("path", "not_directory"));
        }
    }
    let prefix = prefix(target);
    let descriptor = format!("{prefix}SKILL.md");
    if !entries
        .iter()
        .any(|e| e.path == descriptor && e.kind == "blob")
    {
        return Err(invalid("skill", "missing_descriptor"));
    }
    let mut selected = Vec::new();
    let mut ignored = Vec::new();
    let mut folded = HashSet::new();
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in entries {
        let Some(relative) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        // Include the target itself only through its children.
        if relative.is_empty() {
            continue;
        }
        if relative.len() > 4096
            || relative
                .split('/')
                .any(|part| {
                    !segment(part) || part.ends_with('.') || reserved(part)
                })
        {
            return Err(invalid("path", "unsafe"));
        }
        if !matches!(
            (entry.kind.as_str(), entry.mode.as_str()),
            ("tree", "040000") | ("blob", "100644" | "100755")
        ) {
            return Err(invalid("path", "link"));
        }
        if !folded.insert(relative.to_ascii_lowercase()) {
            return Err(invalid("path", "duplicate"));
        }
        // Nested skills are rejected anywhere, including directories the
        // whitelist ignores.
        if relative != "SKILL.md"
            && relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
        {
            return Err(invalid("skill", "nested_skill"));
        }
        if !whitelist(relative) {
            if entry.kind != "tree" {
                if ignored.len() == MAX_ENTRIES {
                    return Err(invalid("skill", "too_many_ignored_files"));
                }
                ignored.push(relative.to_owned());
            }
            continue;
        }
        count += 1;
        if count > MAX_ENTRIES {
            return Err(invalid("skill", "too_many_files"));
        }
        if !package_path(relative) {
            return Err(invalid("path", "unsafe"));
        }
        if entry.kind == "tree" && entry.mode == "040000" {
            continue;
        }
        if matches!(relative, "scripts" | "references" | "assets") {
            return Err(invalid("path", "not_directory"));
        }
        if entry.kind != "blob"
            || !matches!(entry.mode.as_str(), "100644" | "100755")
        {
            return Err(invalid("path", "link"));
        }
        let size = entry
            .size
            .ok_or_else(|| invalid("response", "missing_file_size"))?;
        let cap = file_limit(relative);
        if size > cap as u64 {
            return Err(invalid("file", "too_large").with_limit(cap as u64));
        }
        total += size;
        if total > MAX_TOTAL as u64 {
            return Err(
                invalid("skill", "too_large").with_limit(MAX_TOTAL as u64),
            );
        }
        selected.push(entry);
    }
    Ok((selected, ignored))
}

fn candidates(entries: &[TreeEntry]) -> Result<Vec<Vec<String>>, KitError> {
    let mut out = Vec::new();
    for e in entries {
        if e.path.rsplit('/').next() != Some("SKILL.md") || e.kind != "blob" {
            continue;
        }
        let mut parts: Vec<String> = e.path
            .split('/')
            .map(str::to_owned)
            .collect();
        parts.pop();
        if repo_path(&parts) && selected(entries, &parts).is_ok() {
            if out.len() == MAX_ENTRIES {
                return Err(invalid("repository", "too_many_skills"));
            }
            out.push(parts);
        }
    }
    Ok(out)
}

fn choose_target(
    entries: &[TreeEntry],
    location: &GithubLocation,
    path: Vec<String>,
) -> Result<Vec<String>, KitError> {
    if location.target != Target::Repository {
        return Ok(path);
    }
    if entries
        .iter()
        .filter(|e| {
            e.path.rsplit('/').next() == Some("SKILL.md") && e.kind == "blob"
        })
        .count()
        > 1
    {
        return Err(invalid("repository", "multiple_skills"));
    }
    let mut found = candidates(entries)?;
    if found.len() != 1 {
        return Err(invalid(
            "repository",
            if found.is_empty() {
                "no_skills"
            } else {
                "multiple_skills"
            },
        ));
    }
    found
        .pop()
        .ok_or_else(|| invalid("repository", "no_skills"))
}

fn origin(
    location: &GithubLocation,
    commit: &str,
    target: &[String],
) -> String {
    // A descriptor permalink also expresses an explicitly selected root skill.
    format!(
        "https://github.com/{}/{}/blob/{}/{}SKILL.md",
        location.owner,
        location.repo,
        commit,
        prefix(target)
    )
}

fn file_limit(path: &str) -> usize {
    match path.rsplit('/').next() {
        Some("SKILL.md") => 64 * 1024,
        Some("LICENSE" | "LICENSE.md" | "LICENSE.txt")
            if !path.contains('/') => 32 * 1024,
        _ => MAX_FILE,
    }
}

async fn download(
    http: &Client,
    location: &GithubLocation,
    commit: &str,
    path: &str,
    limit: usize,
) -> Result<Vec<u8>, KitError> {
    let mut url =
        Url::parse("https://raw.githubusercontent.com")
            .map_err(|_| invalid("url", "malformed"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| invalid("url", "malformed"))?;
        segments
            .push(&location.owner)
            .push(&location.repo)
            .push(commit);
        for part in path.split('/') {
            segments.push(part);
        }
    }
    response_bytes(
        http.get(url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(network)?,
        limit,
    )
    .await
}

fn zip_package(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, KitError> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files {
        zip.start_file(path, zip::write::SimpleFileOptions::default())
            .map_err(KitError::internal)?;
        zip.write_all(bytes).map_err(KitError::internal)?;
    }
    let bytes = zip.finish().map_err(KitError::internal)?.into_inner();
    if bytes.len() > MAX_ZIP {
        return Err(invalid("zip", "too_large").with_limit(MAX_ZIP as u64));
    }
    Ok(bytes)
}

/// Searches the real Claude Plugins registry, or discovers skills at a GitHub
/// URL.
///
/// # Errors
/// Returns invalid input, bounded-response, upstream/rate-limit, or network
/// errors. A GitHub repository URL returns all valid candidates, never an
/// arbitrary first skill.
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
            let directory: Vec<String> =
                if item.metadata.directory_path.is_empty() {
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
            // Registry directoryPath is explicit; require the source URL to
            // name that exact directory.
                || location.tail[location.tail.len() - directory.len()..]
                    != directory
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
                name: dir
                    .last()
                    .cloned()
                    .unwrap_or_else(|| location.repo.clone()),
                description: "GitHub skill".into(),
                origin: url.clone(),
                url,
            }
        })
        .collect())
}

/// Resolves a GitHub URL to one immutable commit and downloads only its skill
/// whitelist.
///
/// # Errors
/// Rejects ambiguous refs, multiple repository-root candidates, unsafe paths,
/// links, truncated trees, excessive files/bytes, and upstream/network
/// failures. A directory must contain its own `SKILL.md`; callers cannot select
/// arbitrary repository files.
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
        let bytes = download(
            &http,
            &location,
            &commit,
            &entry.path,
            file_limit(relative),
        )
        .await?;
        total += bytes.len();
        if total > MAX_TOTAL {
            return Err(invalid("skill", "too_large")
                .with_limit(MAX_TOTAL as u64));
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
