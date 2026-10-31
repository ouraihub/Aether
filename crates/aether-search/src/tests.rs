#![expect(clippy::unwrap_used, reason = "failed assertions fail the test")]
use super::*;
use crate::{
    Compression, SearchHit,
    execution::ResolvedConfig,
    policy::{check_blacklist, cutoff, process_hits},
    vendor,
};
use serde_json::json;
use zeroize::Zeroizing;
#[test]
fn each_provider_preserves_its_result_content_and_rejects_bad_shape() {
    let cases = [
        (
            "zhipu",
            json!({
                "search_result": [{
                    "title": "A",
                    "link": "https://example.com",
                    "content": "body",
                }],
            }),
        ),
        (
            "tavily",
            json!({
                "results": [{
                    "title": "A",
                    "url": "https://example.com",
                    "content": "body",
                }],
            }),
        ),
        (
            "searxng",
            json!({
                "results": [{
                    "title": "A",
                    "url": "https://example.com",
                    "snippet": "body",
                }],
            }),
        ),
        (
            "exa",
            json!({
                "results": [{
                    "title": "A",
                    "url": "https://example.com",
                    "text": "body",
                }],
            }),
        ),
        (
            "bocha",
            json!({
                "data": {"webPages": {"value": [{
                    "name": "A",
                    "url": "https://example.com",
                    "summary": "body",
                }]}},
            }),
        ),
        (
            "querit",
            json!({
                "results": {"result": [{
                    "title": "A",
                    "url": "https://example.com",
                    "snippet": "body",
                }]},
            }),
        ),
        (
            "brave",
            json!({
                "web": {"results": [{
                    "title": "A",
                    "url": "https://example.com",
                    "description": "body",
                }]},
            }),
        ),
        (
            "parallel",
            json!({
                "results": [{
                    "title": "A",
                    "url": "https://example.com",
                    "excerpts": ["body"],
                }],
            }),
        ),
        (
            "jina",
            json!({
                "data": [{
                    "title": "A",
                    "url": "https://example.com",
                    "description": "body",
                }],
            }),
        ),
        (
            "firecrawl",
            json!({
                "data": {"web": [{
                    "title": "A",
                    "url": "https://example.com",
                    "markdown": "body",
                }]},
            }),
        ),
    ];
    for (provider, value) in cases {
        let hits = vendor::parse_hits(provider, &value, 5).unwrap();
        assert_eq!(
            hits,
            vec![SearchHit {
                title: "A".into(),
                url: "https://example.com".into(),
                snippet: "body".into()
            }],
            "{provider}"
        );
        assert!(
            vendor::parse_hits(provider, &json!({"error": "bad"}), 5).is_err()
        );
    }
    let body = concat!(
        "data: {\"result\":{\"content\":[{\"text\":\"Title: A\\n",
        "URL: https://example.com\\nText: line one\\nline two\"}]}}\n\n",
    );
    assert_eq!(
        vendor::parse_mcp(body, 5).unwrap()[0].snippet,
        "line one\nline two"
    );
    assert!(vendor::parse_mcp("{\"error\":{\"message\":\"bad\"}}", 5).is_err());
}
#[test]
fn mcp_highlights_keep_paragraphs_between_results() {
    let body = concat!(
        "{\"result\":{\"content\":[{\"text\":\"Title: First\\nURL: ",
        "https://example.com/first\\nHighlights:\\nIntro\\n\\n# Body\\nDetails\\n\\n",
        "Title: Second\\nURL: https://example.com/second\\nText: Last\"}]}}",
    )
    .to_string();
    assert_eq!(
        vendor::parse_mcp(&body, 5).unwrap(),
        vec![
            SearchHit {
                title: "First".into(),
                url: "https://example.com/first".into(),
                snippet: "Intro\n\n# Body\nDetails".into()
            },
            SearchHit {
                title: "Second".into(),
                url: "https://example.com/second".into(),
                snippet: "Last".into()
            },
        ]
    );
}
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[test]
fn scoped_keys_are_ciphertext_masked_bounded_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("search.db");
    let store = Store::open(&file, AddressPolicy::default()).unwrap();
    let settings = store
        .save_key(
            SearchCap::Search,
            "querit",
            "  private-credential-alpha  ",
            None,
        )
        .unwrap();
    let provider = settings
        .providers
        .iter()
        .find(|p| p.id == "querit")
        .unwrap();
    let id = provider.keys[0].id.clone();
    assert!(
        !serde_json::to_string(&settings)
            .unwrap()
            .contains("private-credential-alpha")
    );
    assert_eq!(
        store.reveal(SearchCap::Search, "querit", &id).unwrap(),
        "private-credential-alpha"
    );
    assert!(store.reveal(SearchCap::Fetch, "querit", &id).is_err());
    assert!(
        store
            .save_key(SearchCap::Fetch, "querit", "replacement", Some(&id))
            .is_err()
    );
    assert!(store.remove_key(SearchCap::Fetch, "querit", &id).is_err());
    assert!(
        store
            .save_key(SearchCap::Search, "querit", "   ", Some(&id))
            .is_err()
    );
    assert!(
        store
            .save_key(
                SearchCap::Search,
                "querit",
                &"🙂".repeat(2049),
                Some(&id),
            )
    );
    store
        .save_key(SearchCap::Search, "querit", &"🙂".repeat(2048), Some(&id))
        .unwrap();
    store
        .save_key(
            SearchCap::Search,
            "querit",
            "private-credential-alpha",
            Some(&id),
        )
        .unwrap();
    let cipher = Store::keys(
        &store.lock().unwrap(),
        SearchCap::Search,
        "querit",
    )
    .unwrap()[0]
    .1
    .clone();
    assert!(!cipher.windows(24).any(|w| w == b"private-credential-alpha"));
    store
        .save_key(SearchCap::Search, "querit", "replacement-secret", Some(&id))
        .unwrap();
    for _ in 1..16 {
        store
            .save_key(SearchCap::Search, "querit", "extra-secret", None)
            .unwrap();
    }
    assert!(
        store
            .save_key(SearchCap::Search, "querit", "seventeenth", None)
            .is_err()
    );
    let prefs = SearchPreferences {
        max_results: 50,
        compression: Compression::Cutoff,
        cutoff_limit: 20000,
        blacklist: vec!["example.com".into()],
        model_tools_preferred: true,
    };
    store.save_preferences(prefs).unwrap();
    drop(store);
    let store = Store::open(&file, AddressPolicy::default()).unwrap();
    assert_eq!(
        store.reveal(SearchCap::Search, "querit", &id).unwrap(),
        "replacement-secret"
    );
    assert!(store.preferences().unwrap().model_tools_preferred);
    assert_eq!(
        store
            .settings(SearchCap::Fetch)
            .unwrap()
            .preferences
            .max_results,
        50
    );
    store.remove_key(SearchCap::Search, "querit", &id).unwrap();
    assert!(store.reveal(SearchCap::Search, "querit", &id).is_err());
}
#[tokio::test]
async fn real_local_http_search_honors_basic_password_and_result_limit() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = vec![0; 8192];
        let n = stream.read(&mut bytes).await.unwrap();
        let request = String::from_utf8_lossy(&bytes[..n]).to_string();
        let body = concat!(
            "{\"results\":[{\"title\":\"A\",\
             \"url\":\"https://example.com\",",
            "\"content\":\"body\"},{\"title\":\"B\",\
             \"url\":\"https://other.com\",",
            "\"content\":\"other\"}]}",
        );
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len(),
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        request
    });
    let config = ResolvedConfig {
        provider: "searxng".into(),
        base_url: format!("http://127.0.0.1:{port}"),
        api_keys: vec![Zeroizing::new("password".into())],
        basic_auth_username: Some("user".into()),
        preferences: SearchPreferences {
            max_results: 1,
            ..SearchPreferences::default()
        },
        address_policy: AddressPolicy::default(),
    };
    let result = crate::search::search(config, "hello ?/").await.unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].snippet, "body");
    let request = server.await.unwrap().to_lowercase();
    assert!(request.contains("authorization: basic dxnlcjpwyxnzd29yza=="));
    assert!(request.starts_with("get /search?"));
}

#[test]
fn preferences_reject_invalid_rules_and_filter_before_unicode_cutoff() {
    let mut prefs = SearchPreferences::default();
    for rule in [
        "",
        "https://user@host/path",
        "ftp://example.com/*",
        "example.com/path",
        "bad host",
        "[a-z].com",
        "https://example.com:99999/*",
    ] {
        prefs.blacklist = vec![rule.into()];
        assert!(validate_preferences(&prefs).is_err(), "{rule}");
    }
    prefs.blacklist = vec![
        "EXAMPLE.com".into(),
        "*.blocked.test".into(),
        "*://*.site.test/Private?*".into(),
    ];
    validate_preferences(&prefs).unwrap();
    let body = "🙂界".repeat(1500);
    let mut hits = [
        "https://example.com/a",
        "https://a.example.com/a",
        "https://notexample.com",
        "https://a.blocked.test/",
        "https://a.site.test/PrivateX",
        "https://a.site.test/privateX",
    ]
    .into_iter()
    .map(|url| SearchHit {
        title: "title".into(),
        url: url.into(),
        snippet: body.clone(),
    })
    .collect::<Vec<_>>();
    process_hits(&mut hits, &prefs);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].snippet, body);
    prefs.compression = Compression::Cutoff;
    prefs.cutoff_limit = 101;
    process_hits(&mut hits, &prefs);
    assert_eq!(hits[0].snippet.chars().count(), 101);
    assert!(hits[0].snippet.ends_with('🙂'));
    assert_eq!(hits[0].title, "title");
    prefs.max_results = 0;
    assert!(validate_preferences(&prefs).is_err());
    prefs.max_results = 51;
    assert!(validate_preferences(&prefs).is_err());
    prefs.max_results = 1;
    for n in [99, 20001] {
        prefs.cutoff_limit = n;
        assert!(validate_preferences(&prefs).is_err());
    }
    prefs.cutoff_limit = 100;
    prefs.blacklist = vec!["example.com".into(); 33];
    assert!(validate_preferences(&prefs).is_err());
    assert!(serde_json::from_value::<SearchPreferences>(json!({
        "maxResults": 5,
        "compression": "regex",
        "cutoffLimit": 2000,
        "blacklist": [],
        "modelToolsPreferred": false,
    }))
    .is_err());
    let parsed = vendor::parse_hits(
        "tavily",
        &json!({
            "results": [{"title":"A","url":"https://good.test","content":body}],
        }),
        5,
    )
    .unwrap();
    assert_eq!(parsed[0].snippet.chars().count(), 3000);
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn real_keys_retry_only_explicit_status_and_rotate_next_start() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for status in [401, 200, 200, 500, 429, 403] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 8192];
            let n = stream.read(&mut bytes).await.unwrap();
            requests.push(String::from_utf8_lossy(&bytes[..n]).to_lowercase());
            let body = if status == 200 {
                concat!(
                    "{\"results\":[{\"title\":\"A\",\"url\":\"https://example.com\",",
                    "\"text\":\"body\"}]}",
                )
            } else {
                "rejected"
            };
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Status\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n{body}",
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        requests
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("search.db"),
        AddressPolicy::default(),
    )
    .unwrap();
    store
        .save(SearchSave {
            capability: SearchCap::Search,
            provider: "exa".into(),
            base_url: Some(format!("http://{addr}")),
            basic_auth_username: None,
        })
        .unwrap();
    store
        .save_key(SearchCap::Search, "exa", "first-key", None)
        .unwrap();
    store
        .save_key(SearchCap::Search, "exa", "second-key", None)
        .unwrap();
    store.set_default(SearchCap::Search, "exa").unwrap();
    assert_eq!(store.search("query").await.unwrap().hits.len(), 1);
    assert_eq!(
        store.check(SearchCap::Search, "exa").await.unwrap().count,
        1
    );
    assert_eq!(
        store.search("query").await.unwrap_err().details().status(),
        Some(500)
    );
    assert_eq!(
        store.search("query").await.unwrap_err().details().status(),
        Some(403)
    );
    let requests = server.await.unwrap();
    for (request, key) in requests.iter().zip([
        "first-key",
        "second-key",
        "second-key",
        "first-key",
        "second-key",
        "first-key",
    ]) {
        assert!(request.contains(&format!("x-api-key: {key}")));
    }
}


#[path = "tests_network.rs"]
mod tests_network;

include!("tests_extra.rs");
