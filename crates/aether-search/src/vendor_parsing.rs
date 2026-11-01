use crate::{SearchHit, SearchPage};
use aether_support::{KitError, KitErrorCode, sse::SseParser};
use serde_json::Value;
use std::collections::HashSet;

fn upstream(message: &str) -> KitError {
    KitError::new(KitErrorCode::Upstream, message)
}

fn field(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| match value.get(*key)? {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Value::Array(parts) => {
                let s = parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (!s.is_empty()).then_some(s)
            }
            Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Object(_) => None,
        })
        .unwrap_or_default()
}

pub(crate) fn clip(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn collect(items: impl Iterator<Item = (String, String, String)>, limit: usize) -> Vec<SearchHit> {
    let mut seen = HashSet::new();
    items
        .filter_map(|(title, url, snippet)| {
            let parsed = reqwest::Url::parse(&url).ok()?;
            if !matches!(parsed.scheme(), "https" | "http")
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || !seen.insert(url.clone())
            {
                return None;
            }
            Some(SearchHit {
                title: clip(if title.is_empty() { &url } else { &title }, 500),
                url,
                snippet,
            })
        })
        .take(limit)
        .collect()
}

pub(crate) fn parse_hits(
    provider: &str,
    value: &Value,
    limit: usize,
) -> Result<Vec<SearchHit>, KitError> {
    let (paths, titles, bodies): (&[&str], &[&str], &[&str]) = match provider {
        "zhipu" => (&["/search_result"], &["title"], &["content"]),
        "bocha" => (
            &["/data/webPages/value"],
            &["name"],
            &["summary", "snippet"],
        ),
        "querit" => (
            &["/results/result"],
            &["title", "site_name", "snippet"],
            &["snippet"],
        ),
        "brave" => (&["/web/results"], &["title"], &["description"]),
        "parallel" => (&["/results"], &["title"], &["excerpts"]),
        "jina" => (
            &["/data", "/results"],
            &["title"],
            &["content", "description"],
        ),
        "firecrawl" => (&["/data/web"], &["title"], &["markdown", "description"]),
        _ => (&["/results"], &["title"], &["content", "text", "snippet"]),
    };
    let items = paths
        .iter()
        .find_map(|p| value.pointer(p).and_then(Value::as_array))
        .ok_or_else(|| upstream("invalid search results shape"))?;
    Ok(collect(
        items.iter().map(|v| {
            (
                field(v, titles),
                field(v, &["url", "link"]),
                field(v, bodies),
            )
        }),
        limit,
    ))
}

fn mcp_text(body: &str) -> Result<Option<String>, KitError> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Ok(None);
    };
    if value.get("error").is_some() || value.pointer("/result/isError") == Some(&Value::Bool(true))
    {
        return Err(upstream("MCP search rejected"));
    }
    Ok(value
        .pointer("/result/content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|v| v.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        }))
}

pub(crate) fn parse_mcp(body: &str, limit: usize) -> Result<Vec<SearchHit>, KitError> {
    let mut parser = SseParser::new();
    let mut events = parser.feed(body.as_bytes());
    events.extend(parser.finish());
    let mut texts = Vec::new();
    for event in events {
        if let Some(t) = mcp_text(&event.data)? {
            texts.push(t);
        }
    }
    if texts.is_empty()
        && let Some(t) = mcp_text(body)?
    {
        texts.push(t);
    }
    if texts.is_empty() {
        return Err(upstream("invalid MCP search response"));
    }
    let joined = texts.join("\n\n");
    Ok(collect(
        joined.split("\nTitle:").enumerate().map(|(index, block)| {
            let (mut title, mut url, mut snippets) = (String::new(), String::new(), Vec::new());
            let mut content = false;
            for (line_index, line) in block.lines().enumerate() {
                if index > 0 && line_index == 0 {
                    title = line.trim().to_owned();
                    continue;
                }
                if content {
                    snippets.push(line);
                } else if let Some(v) = line.strip_prefix("Title:") {
                    title = v.trim().to_owned();
                } else if let Some(v) = line.strip_prefix("URL:") {
                    url = v.trim().to_owned();
                } else if let Some(v) = line
                    .strip_prefix("Text:")
                    .or_else(|| line.strip_prefix("Highlights:"))
                {
                    content = true;
                    snippets.push(v.trim_start());
                }
            }
            (title, url, snippets.join("\n").trim().to_owned())
        }),
        limit,
    ))
}

pub(crate) fn page(provider: &str, value: &Value, url: &str) -> Result<SearchPage, KitError> {
    let data = if provider == "querit" {
        value
            .pointer("/results")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .or_else(|| {
                value
                    .pointer("/data")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
            })
            .unwrap_or(value)
    } else {
        value.get("data").unwrap_or(value)
    };
    let meta = data.get("metadata").unwrap_or(data);
    let title = {
        let title = field(meta, &["title"]);
        if title.is_empty() {
            field(data, &["title"])
        } else {
            title
        }
    };
    let content = field(data, &["markdown", "content", "text"]);
    if content.trim().is_empty() {
        return Err(upstream("webpage has no content"));
    }
    let source = {
        let source = field(meta, &["sourceURL", "sourceUrl", "url", "link"]);
        if source.is_empty() {
            field(data, &["url", "link"])
        } else {
            source
        }
    };
    let source = if source.is_empty() { url } else { &source };
    Ok(SearchPage {
        title: clip(if title.is_empty() { source } else { &title }, 500),
        url: source.to_owned(),
        content: clip(&content, 24000),
    })
}

pub(crate) fn html(body: &str, url: &str) -> Result<SearchPage, KitError> {
    // ponytail: static HTML text extraction; a readability parser if navigation noise becomes a problem.
    let lower = body.to_ascii_lowercase();
    let title = lower
        .find("<title")
        .and_then(|i| lower[i..].find('>').map(|e| i + e + 1))
        .and_then(|i| {
            lower[i..]
                .find("</title>")
                .map(|e| body[i..i + e].trim().to_owned())
        })
        .unwrap_or_else(|| url.to_owned());
    let mut text = String::new();
    let mut at = 0;
    while at < body.len() {
        if body.as_bytes()[at] == b'<' {
            let end = body[at..]
                .find('>')
                .map(|n| at + n + 1)
                .unwrap_or(body.len());
            let tag = lower[at..end].trim_start_matches('<').trim();
            let noise = ["script", "style", "noscript", "head"]
                .into_iter()
                .find(|name| {
                    tag.starts_with(name)
                        && tag
                            .as_bytes()
                            .get(name.len())
                            .is_none_or(|b| b.is_ascii_whitespace() || *b == b'>')
                });
            if let Some(name) = noise
                && let Some(close) = lower[end..].find(&format!("</{name}>"))
            {
                at = end + close + name.len() + 3;
                continue;
            }
            at = end;
            text.push(' ');
        } else {
            let Some(ch) = body[at..].chars().next() else {
                break;
            };
            text.push(ch);
            at += ch.len_utf8();
        }
    }
    let content = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    if content.is_empty() {
        return Err(upstream("webpage has no content"));
    }
    Ok(SearchPage {
        title: clip(&title, 500),
        url: url.to_owned(),
        content: clip(&content, 24000),
    })
}
