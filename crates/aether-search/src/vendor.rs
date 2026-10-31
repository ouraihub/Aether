//! Provider request shapes, bounded reads and response parsing.
use crate::{SearchCap, SearchHit, SearchPage, execution::Attempt};
use aether_support::{KitError, KitErrorCode, net, sse::SseParser};
use base64::Engine;
use reqwest::{
    RequestBuilder,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};
use zeroize::Zeroizing;
mod vendor_specs;
mod vendor_html;
use vendor_specs::PROVIDERS;

pub(crate) struct Spec {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) website: &'static str,
    pub(crate) key_site: Option<&'static str>,
    pub(crate) basic: bool,
    pub(crate) search: Option<(&'static str, &'static str)>,
    pub(crate) fetch: Option<(&'static str, &'static str)>,
}
pub(crate) fn lookup(
    cap: SearchCap,
    id: &str,
) -> Result<(&'static Spec, &'static str, &'static str), KitError> {
    PROVIDERS
        .iter()
        .find_map(|spec| {
            let config = match cap {
                SearchCap::Search => spec.search,
                SearchCap::Fetch => spec.fetch,
            };
            config
                .filter(|_| spec.id == id)
                .map(|(base, mode)| (spec, base, mode))
        })
        .ok_or_else(|| KitError::bad_request("provider", "unsupported"))
}
pub(crate) fn default_provider(cap: SearchCap) -> &'static str {
    match cap {
        SearchCap::Search => "exa-mcp",
        SearchCap::Fetch => "jina",
    }
}

const BODY_LIMIT: usize = 2 * 1024 * 1024;
const ERROR_LIMIT: usize = 8 * 1024;
fn upstream(message: &str) -> KitError {
    KitError::new(KitErrorCode::Upstream, message)
}
fn header(raw: &str) -> Result<HeaderValue, KitError> {
    let mut value = HeaderValue::from_str(raw)
        .map_err(|_| KitError::bad_request("apiKey", "invalid_header"))?;
    value.set_sensitive(true);
    Ok(value)
}
fn auth(runtime: &Attempt<'_>, mode: &str) -> Result<HeaderMap, KitError> {
    let mut out = HeaderMap::new();
    out.insert(ACCEPT, HeaderValue::from_static("application/json"));
    if mode == "basic" {
        if let Some(user) = runtime.user.filter(|v| !v.is_empty()) {
            let pair = Zeroizing::new(format!(
                "{user}:{}",
                runtime.key.unwrap_or(""),
            ));
            let raw = Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .encode(pair.as_bytes()),
            );
            let authorization = Zeroizing::new(format!(
                "Basic {}",
                raw.as_str(),
            ));
            out.insert(AUTHORIZATION, header(&authorization)?);
        }
    } else if let Some(key) = runtime.key {
        match mode {
            "api" => {
                out.insert("x-api-key", header(key)?);
            }
            "brave" => {
                out.insert("x-subscription-token", header(key)?);
            }
            _ => {
                let authorization = Zeroizing::new(format!("Bearer {key}"));
                out.insert(AUTHORIZATION, header(&authorization)?);
            }
        }
    }
    Ok(out)
}
async fn send(
    request: RequestBuilder,
    runtime: &Attempt<'_>,
) -> Result<String, KitError> {
    read_response(request.send().await.map_err(network)?, runtime.key).await
}
fn network(error: reqwest::Error) -> KitError {
    let timeout = error.is_timeout();
    let blocked = net::is_blocked(&error);
    drop(error);
    if timeout {
        KitError::new(KitErrorCode::Timeout, "search request timed out")
    } else if blocked {
        KitError::bad_request("url", "blocked_address")
    } else {
        KitError::new(KitErrorCode::Network, "search provider unreachable")
    }
}
fn json_body(body: &str) -> Result<Value, KitError> {
    serde_json::from_str(body).map_err(|_| upstream("invalid provider JSON"))
}
fn provider_error(provider: &str, value: &Value) -> Result<(), KitError> {
    let failed = match provider {
        "bocha" => value
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|v| v != 200),
        "querit" => value
            .get("error_code")
            .and_then(Value::as_i64)
            .is_some_and(|v| v != 200),
        "firecrawl" => value.get("success") == Some(&Value::Bool(false)),
        _ => false,
    };
    if failed {
        Err(upstream("provider rejected the request"))
    } else {
        Ok(())
    }
}
pub(crate) async fn search(
    policy: &net::AddressPolicy,
    runtime: &Attempt<'_>,
    query: &str,
) -> Result<Vec<SearchHit>, KitError> {
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let base = runtime.base.trim_end_matches('/');
    let count = runtime.max;
    let path = match runtime.provider {
        "zhipu" | "exa-mcp" => String::new(),
        "tavily" | "exa" | "searxng" => "/search".to_owned(),
        "bocha" => "/v1/web-search".to_owned(),
        "querit" | "parallel" => "/v1/search".to_owned(),
        "brave" => "/res/v1/web/search".to_owned(),
        "jina" => format!("/{}", encode(query)),
        "firecrawl" => "/v2/search".to_owned(),
        _ => return Err(KitError::bad_request("provider", "unsupported")),
    };
    let target = net::check_url(&format!("{base}{path}"), policy)?;
    let request = match runtime.provider {
        "zhipu" => client
            .post(target)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({
                "search_query": query,
                "search_engine": "search_std",
                "search_intent": false,
            })),
        "tavily" => client
            .post(target)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({"query": query, "max_results": count})),
        "searxng" => client
            .get(target)
            .headers(auth(runtime, "basic")?)
            .query(&[("q", query), ("format", "json"), ("language", "auto")]),
        "exa" => client
            .post(target)
            .headers(auth(runtime, "api")?)
            .json(&json!({
                "query": query,
                "numResults": count,
                "contents": {"text": true},
            })),
        "exa-mcp" => {
            let mut headers = auth(runtime, "api")?;
            headers.insert(
                ACCEPT,
                HeaderValue::from_static("application/json, text/event-stream"),
            );
            client
                .post(target)
                .headers(headers)
                .json(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "web_search_exa",
                        "arguments": {
                            "query": query,
                            "type": "auto",
                            "numResults": count,
                            "livecrawl": "fallback",
                        },
                    },
                }))
        }
        "bocha" => client
            .post(target)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({
                "query": query,
                "count": count,
                "exclude": "",
                "summary": true,
            })),
        "querit" => client
            .post(target)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({"query": query, "count": count})),
        "brave" => client
            .get(target)
            .headers(auth(runtime, "brave")?)
            .query(&[("q", query), ("count", &count.min(20).to_string())]),
        "parallel" => client
            .post(target)
            .headers(auth(runtime, "api")?)
            .json(&json!({
                "objective": query,
                "search_queries": [query],
                "advanced_settings": {"max_results": count},
            })),
        "jina" => client.get(target).headers(auth(runtime, "bearer")?),
        "firecrawl" => client
            .post(target)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({
                "query": query,
                "limit": count,
                "scrapeOptions": {"formats": ["markdown"]},
            })),
        _ => return Err(KitError::bad_request("provider", "unsupported")),
    };
    let body = send(request, runtime).await?;
    if runtime.provider == "exa-mcp" {
        return parse_mcp(&body, usize::MAX);
    }
    let value = json_body(&body)?;
    provider_error(runtime.provider, &value)?;
    parse_hits(runtime.provider, &value, usize::MAX)
}
fn encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    out
}
fn field(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| match value.get(*key)? {
            Value::String(s) if !s.trim().is_empty() =>
                Some(s.trim().to_owned()),
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
fn clip(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}
fn collect(
    items: impl Iterator<Item = (String, String, String)>,
    limit: usize,
) -> Vec<SearchHit> {
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
        "firecrawl" => (
            &["/data/web"],
            &["title"],
            &["markdown", "description"],
        ),
        _ => (
            &["/results"],
            &["title"],
            &["content", "text", "snippet"],
        ),
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
    if value.get("error").is_some()
        || value.pointer("/result/isError") == Some(&Value::Bool(true))
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
pub(crate) fn parse_mcp(
    body: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, KitError> {
    let mut parser = SseParser::new();
    let mut events = parser.feed(body.as_bytes());
    events.extend(parser.finish());
    let mut texts = Vec::new();
    for event in events {
        if let Some(t) = mcp_text(&event.data)? {
            texts.push(t)
        }
    }
    if texts.is_empty()
        && let Some(t) = mcp_text(body)?
    {
        texts.push(t)
    }
    if texts.is_empty() {
        return Err(upstream("invalid MCP search response"));
    }
    let joined = texts.join("\n\n");
    Ok(collect(
        joined.split("\nTitle:").enumerate().map(|(index, block)| {
            let (mut title, mut url, mut snippets) = (
                String::new(),
                String::new(),
                Vec::new(),
            );
            let mut content = false;
            for (line_index, line) in block.lines().enumerate() {
                if index > 0 && line_index == 0 {
                    title = line.trim().to_owned();
                    continue;
                }
                if content {
                    snippets.push(line)
                } else if let Some(v) = line.strip_prefix("Title:") {
                    title = v.trim().to_owned()
                } else if let Some(v) = line.strip_prefix("URL:") {
                    url = v.trim().to_owned()
                } else if let Some(v) = line
                    .strip_prefix("Text:")
                    .or_else(|| line.strip_prefix("Highlights:"))
                {
                    content = true;
                    snippets.push(v.trim_start())
                }
            }
            (title, url, snippets.join("\n").trim().to_owned())
        }),
        limit,
    ))
}
pub(crate) async fn fetch(
    policy: &net::AddressPolicy,
    runtime: &Attempt<'_>,
    url: &str,
    prefs: &crate::SearchPreferences,
) -> Result<SearchPage, KitError> {
    let target = net::check_url(url, policy)?;
    crate::policy::check_blacklist(target.as_str(), prefs)?;
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let base = runtime.base.trim_end_matches('/');
    if runtime.provider == "fetch" {
        let mut current = target;
        for count in 0..=5 {
            let response = client
                .get(current.clone())
                .send()
                .await
                .map_err(network)?;
            if response.status().is_redirection() {
                if count == 5 {
                    return Err(upstream("too many redirects"));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| upstream("redirect missing location"))?;
                let next = current
                    .join(location)
                    .map_err(|_| {
                        KitError::bad_request("url", "invalid_redirect")
                    })?;
                current = net::check_url(next.as_str(), policy)?;
                crate::policy::check_blacklist(current.as_str(), prefs)?;
                continue;
            }
            let body = read_response(response, None).await?;
            return vendor_html::html(&body, current.as_str());
        }
        return Err(upstream("too many redirects"));
    }
    net::check_target(url, policy).await?;
    let endpoint = match runtime.provider {
        "jina" => format!("{base}/{url}"),
        "querit" => format!("{base}/v1/contents"),
        "firecrawl" => format!("{base}/v2/scrape"),
        _ => return Err(KitError::bad_request("provider", "unsupported")),
    };
    let checked = net::check_url(&endpoint, policy)?;
    let req = match runtime.provider {
        "jina" => client
            .get(checked)
            .headers(auth(runtime, "bearer")?)
            .header("x-retain-images", "none"),
        "querit" => client
            .post(checked)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({"urls":[url],"format":"markdown","extrasMeta":true})),
        "firecrawl" => client
            .post(checked)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({"url":url,"formats":["markdown"]})),
        _ => return Err(KitError::bad_request("provider", "unsupported")),
    };
    let body = send(req, runtime).await?;
    let value = json_body(&body)?;
    provider_error(runtime.provider, &value)?;
    vendor_html::page(runtime.provider, &value, url)
}
async fn read_response(
    response: reqwest::Response,
    key: Option<&str>,
) -> Result<String, KitError> {
    let status = response.status();
    let mut response = response;
    let mut out = Vec::new();
    let limit = if status.is_success() {
        BODY_LIMIT
    } else {
        ERROR_LIMIT
    };
    if status.is_success()
        && response.content_length().is_some_and(|n| n > limit as u64)
    {
        return Err(upstream("response too large"));
    }
    while let Some(b) = response.chunk().await.map_err(network)? {
        let room = limit.saturating_sub(out.len());
        if b.len() > room {
            if status.is_success() {
                return Err(upstream("response too large"));
            }
            out.extend_from_slice(&b[..room]);
            break;
        }
        out.extend_from_slice(&b)
    }
    let text = String::from_utf8_lossy(&out).into_owned();
    if !status.is_success() {
        return Err(KitError::upstream(status.as_u16(), &text, key.as_slice()));
    }
    Ok(text)
}
        vendor_html::page(runtime.provider, &value, url)

#[cfg(test)]
#[path = "vendor_tests.rs"]
mod tests;
