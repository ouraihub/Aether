//! Provider request shapes and bounded reads.
use crate::{SearchCap, SearchHit, SearchPage, execution::Attempt};
use aether_support::{KitError, KitErrorCode, net};
use base64::Engine;
use reqwest::{
    RequestBuilder,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::time::Duration;
use zeroize::Zeroizing;
#[path = "vendor_parsing.rs"]
mod parsing;
#[cfg(test)]
use parsing::clip;
use parsing::{html, page};
pub(crate) use parsing::{parse_hits, parse_mcp};

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
pub(crate) const PROVIDERS: &[Spec] = &[
    Spec {
        id: "zhipu",
        name: "智谱 Zhipu",
        description: "GLM Web Search",
        website: "https://docs.bigmodel.cn/cn/guide/tools/web-search",
        key_site: Some(
            "https://www.bigmodel.cn/invite?icode=X2d48dEyAbBOUJCLpZmYJUjPr3uHog9F4g5tjuOUqno%3D",
        ),
        basic: false,
        search: Some((
            "https://open.bigmodel.cn/api/paas/v4/web_search",
            "required",
        )),
        fetch: None,
    },
    Spec {
        id: "tavily",
        name: "Tavily",
        description: "Search optimized for LLMs",
        website: "https://tavily.com",
        key_site: Some("https://app.tavily.com/home"),
        basic: false,
        search: Some(("https://api.tavily.com", "required")),
        fetch: None,
    },
    Spec {
        id: "searxng",
        name: "SearXNG",
        description: "Self-hosted meta search",
        website: "https://docs.searxng.org",
        key_site: None,
        basic: true,
        search: Some(("http://localhost:8080", "none")),
        fetch: None,
    },
    Spec {
        id: "exa",
        name: "Exa",
        description: "Neural search API",
        website: "https://exa.ai",
        key_site: Some("https://dashboard.exa.ai/api-keys"),
        basic: false,
        search: Some(("https://api.exa.ai", "required")),
        fetch: None,
    },
    Spec {
        id: "exa-mcp",
        name: "Exa MCP",
        description: "MCP search; anonymous access subject to provider limits",
        website: "https://exa.ai",
        key_site: Some("https://dashboard.exa.ai/api-keys"),
        basic: false,
        search: Some(("https://mcp.exa.ai/mcp", "optional")),
        fetch: None,
    },
    Spec {
        id: "bocha",
        name: "博查 Bocha",
        description: "Chinese web search",
        website: "https://bochaai.com",
        key_site: Some("https://open.bochaai.com/overview"),
        basic: false,
        search: Some(("https://api.bochaai.com", "required")),
        fetch: None,
    },
    Spec {
        id: "querit",
        name: "Querit",
        description: "Search and page content",
        website: "https://querit.ai",
        key_site: Some("https://www.querit.ai/en/dashboard/api-keys"),
        basic: false,
        search: Some(("https://api.querit.ai", "required")),
        fetch: Some(("https://api.querit.ai", "required")),
    },
    Spec {
        id: "brave",
        name: "Brave Search",
        description: "Independent search index",
        website: "https://brave.com/search/api/",
        key_site: Some("https://api-dashboard.search.brave.com/app/keys"),
        basic: false,
        search: Some(("https://api.search.brave.com", "required")),
        fetch: None,
    },
    Spec {
        id: "parallel",
        name: "Parallel",
        description: "Search with relevant page excerpts",
        website: "https://parallel.ai",
        key_site: Some("https://platform.parallel.ai"),
        basic: false,
        search: Some(("https://api.parallel.ai", "required")),
        fetch: None,
    },
    Spec {
        id: "jina",
        name: "Jina",
        description: "Reader search and webpage extraction",
        website: "https://jina.ai/reader",
        key_site: Some("https://jina.ai"),
        basic: false,
        search: Some(("https://s.jina.ai", "required")),
        fetch: Some(("https://r.jina.ai", "optional")),
    },
    Spec {
        id: "firecrawl",
        name: "Firecrawl",
        description: "Web search and Markdown extraction",
        website: "https://firecrawl.dev",
        key_site: Some("https://firecrawl.dev/app/api-keys"),
        basic: false,
        search: Some(("https://api.firecrawl.dev", "optional")),
        fetch: Some(("https://api.firecrawl.dev", "optional")),
    },
    Spec {
        id: "fetch",
        name: "Direct fetch",
        description: "Read static public HTML without a key",
        website: "",
        key_site: None,
        basic: false,
        search: None,
        fetch: Some(("", "none")),
    },
];
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
            let pair = Zeroizing::new(format!("{user}:{}", runtime.key.unwrap_or("")));
            let raw =
                Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(pair.as_bytes()));
            let authorization = Zeroizing::new(format!("Basic {}", raw.as_str()));
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
async fn send(request: RequestBuilder, runtime: &Attempt<'_>) -> Result<String, KitError> {
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
            "zhipu" => client.post(target).headers(auth(runtime,"bearer")?).json(&json!({"search_query":query,"search_engine":"search_std","search_intent":false})),
            "tavily" => client.post(target).headers(auth(runtime,"bearer")?).json(&json!({"query":query,"max_results":count})),
            "searxng" => client.get(target).headers(auth(runtime,"basic")?).query(&[("q",query),("format","json"),("language","auto")]),
            "exa" => client.post(target).headers(auth(runtime,"api")?).json(&json!({"query":query,"numResults":count,"contents":{"text":true}})),
            "exa-mcp" => { let mut h=auth(runtime,"api")?; h.insert(ACCEPT,HeaderValue::from_static("application/json, text/event-stream")); client.post(target).headers(h).json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"web_search_exa","arguments":{"query":query,"type":"auto","numResults":count,"livecrawl":"fallback"}}})) },
            "bocha" => client.post(target).headers(auth(runtime,"bearer")?).json(&json!({"query":query,"count":count,"exclude":"","summary":true})),
            "querit" => client.post(target).headers(auth(runtime,"bearer")?).json(&json!({"query":query,"count":count})),
            "brave" => client.get(target).headers(auth(runtime,"brave")?).query(&[("q",query),("count",&count.min(20).to_string())]),
            "parallel" => client.post(target).headers(auth(runtime,"api")?).json(&json!({"objective":query,"search_queries":[query],"advanced_settings":{"max_results":count}})),
            "jina" => client.get(target).headers(auth(runtime,"bearer")?),
            "firecrawl" => client.post(target).headers(auth(runtime,"bearer")?).json(&json!({"query":query,"limit":count,"scrapeOptions":{"formats":["markdown"]}})),
            _ => return Err(KitError::bad_request("provider","unsupported")),
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
            let response = client.get(current.clone()).send().await.map_err(network)?;
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
                    .map_err(|_| KitError::bad_request("url", "invalid_redirect"))?;
                current = net::check_url(next.as_str(), policy)?;
                crate::policy::check_blacklist(current.as_str(), prefs)?;
                continue;
            }
            let body = read_response(response, None).await?;
            return html(&body, current.as_str());
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
    page(runtime.provider, &value, url)
}
async fn read_response(response: reqwest::Response, key: Option<&str>) -> Result<String, KitError> {
    let status = response.status();
    let mut response = response;
    let mut out = Vec::new();
    let limit = if status.is_success() {
        BODY_LIMIT
    } else {
        ERROR_LIMIT
    };
    if status.is_success() && response.content_length().is_some_and(|n| n > limit as u64) {
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

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed assertion fails the test")]
    use super::*;

    #[test]
    fn page_parsers_preserve_unicode_and_strip_executable_noise() {
        let expected = "正文🙂".repeat(500);
        for (provider, value) in [
            ("jina", json!({"data":{"title":"Page","content":expected}})),
            (
                "firecrawl",
                json!({"data":{"metadata":{"title":"Page"},"markdown":expected}}),
            ),
            (
                "querit",
                json!({"results":[{"title":"Page","content":expected}]}),
            ),
        ] {
            assert_eq!(
                page(provider, &value, "https://example.com")
                    .unwrap()
                    .content,
                expected
            );
        }
        let parsed = html("<html><head><title>Page</title><style>hidden</style></head><body>正文🙂<script>secret()</script>&amp; text</body></html>", "https://example.com").unwrap();
        assert_eq!(parsed.title, "Page");
        assert!(parsed.content.contains("正文🙂") && parsed.content.contains("& text"));
        assert!(!parsed.content.contains("secret()") && !parsed.content.contains("hidden"));
        assert_eq!(clip(&"🙂".repeat(2000), 1000).chars().count(), 1000);
    }

    #[tokio::test]
    async fn declared_oversized_body_is_rejected_before_download() {
        use aether_testkit::{Exhausted, Reply, Server};
        let mut server = Server::script(
            vec![Reply::raw(
                "HTTP/1.1 200 OK\r\nContent-Length: 3000000\r\nConnection: close\r\n\r\n",
            )],
            Exhausted::Reply(Reply::status(500, "unexpected request")),
        )
        .await;
        let result = crate::search::search(
            crate::execution::ResolvedConfig {
                provider: "searxng".into(),
                base_url: server.url(),
                api_keys: Vec::new(),
                basic_auth_username: None,
                preferences: crate::SearchPreferences::default(),
                address_policy: net::AddressPolicy::default(),
            },
            "query",
        )
        .await;
        assert_eq!(result.unwrap_err().message(), "response too large");
        server.request().await;
    }
}
