#![expect(clippy::unwrap_used, reason = "failed assertions fail the test")]
use super::*;
use serde_json::json;
#[test]
fn each_provider_preserves_its_result_content_and_rejects_bad_shape() {
    let cases = [
        (
            "zhipu",
            json!({"search_result":[{"title":"A","link":"https://example.com","content":"body"}]}),
        ),
        (
            "tavily",
            json!({"results":[{"title":"A","url":"https://example.com","content":"body"}]}),
        ),
        (
            "searxng",
            json!({"results":[{"title":"A","url":"https://example.com","snippet":"body"}]}),
        ),
        (
            "exa",
            json!({"results":[{"title":"A","url":"https://example.com","text":"body"}]}),
        ),
        (
            "bocha",
            json!({"data":{"webPages":{"value":[{"name":"A","url":"https://example.com","summary":"body"}]}}}),
        ),
        (
            "querit",
            json!({"results":{"result":[{"title":"A","url":"https://example.com","snippet":"body"}]}}),
        ),
        (
            "brave",
            json!({"web":{"results":[{"title":"A","url":"https://example.com","description":"body"}]}}),
        ),
        (
            "parallel",
            json!({"results":[{"title":"A","url":"https://example.com","excerpts":["body"]}]}),
        ),
        (
            "jina",
            json!({"data":[{"title":"A","url":"https://example.com","description":"body"}]}),
        ),
        (
            "firecrawl",
            json!({"data":{"web":[{"title":"A","url":"https://example.com","markdown":"body"}]}}),
        ),
    ];
    for (provider, value) in cases {
        let hits = protocol::parse_hits(provider, &value, 5).unwrap();
        assert_eq!(
            hits,
            vec![SearchHit {
                title: "A".into(),
                url: "https://example.com".into(),
                snippet: "body".into()
            }],
            "{provider}"
        );
        assert!(protocol::parse_hits(provider, &json!({"error":"bad"}), 5).is_err());
    }
    let body = "data: {\"result\":{\"content\":[{\"text\":\"Title: A\\nURL: https://example.com\\nText: line one\\nline two\"}]}}\n\n";
    assert_eq!(
        protocol::parse_mcp(body, 5).unwrap()[0].snippet,
        "line one\nline two"
    );
    assert!(protocol::parse_mcp("{\"error\":{\"message\":\"bad\"}}", 5).is_err());
}
#[test]
fn mcp_highlights_keep_paragraphs_between_results() {
    let body = json!({"result":{"content":[{"text":"Title: First\nURL: https://example.com/first\nHighlights:\nIntro\n\n# Body\nDetails\n\nTitle: Second\nURL: https://example.com/second\nText: Last"}]}}).to_string();
    assert_eq!(
        protocol::parse_mcp(&body, 5).unwrap(),
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
            .save_key(SearchCap::Search, "querit", &"🙂".repeat(2049), Some(&id))
            .is_err()
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
    let cipher = Store::keys(&store.lock().unwrap(), SearchCap::Search, "querit").unwrap()[0]
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
        let body = "{\"results\":[{\"title\":\"A\",\"url\":\"https://example.com\",\"content\":\"body\"},{\"title\":\"B\",\"url\":\"https://other.com\",\"content\":\"other\"}]}";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        request
    });
    let runtime = Runtime {
        provider: "searxng".into(),
        base: format!("http://127.0.0.1:{port}"),
        key: Some(Zeroizing::new("password".into())),
        user: Some("user".into()),
        max: 1,
    };
    let result = protocol::search(&AddressPolicy::default(), &runtime, "hello ?/")
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].snippet, "body");
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
    assert!(serde_json::from_value::<SearchPreferences>(json!({"maxResults":5,"compression":"regex","cutoffLimit":2000,"blacklist":[],"modelToolsPreferred":false})).is_err());
    let parsed = protocol::parse_hits(
        "tavily",
        &json!({"results":[{"title":"A","url":"https://good.test","content":body}]}),
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
                "{\"results\":[{\"title\":\"A\",\"url\":\"https://example.com\",\"text\":\"body\"}]}"
            } else {
                "rejected"
            };
            stream.write_all(format!("HTTP/1.1 {status} Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
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

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn fetch_retries_keys_but_network_and_cancel_do_not() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{Duration, timeout},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (no_retry_tx, no_retry_rx) = tokio::sync::oneshot::channel();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        for (status, key) in [(403, "one-key"), (200, "two-key")] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 8192];
            let n = stream.read(&mut bytes).await.unwrap();
            assert!(
                String::from_utf8_lossy(&bytes[..n])
                    .to_lowercase()
                    .contains(&format!("authorization: bearer {key}"))
            );
            let body = "{\"data\":{\"content\":\"page body\"}}";
            stream.write_all(format!("HTTP/1.1 {status} Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        drop(stream);
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
        no_retry_tx.send(()).unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        sent_tx.send(()).unwrap();
        // Wait for cancellation to close the request, then ensure no next key was sent.
        assert_eq!(stream.read(&mut bytes).await.unwrap(), 0);
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
    store
        .save(SearchSave {
            capability: SearchCap::Fetch,
            provider: "jina".into(),
            base_url: Some(format!("http://{addr}")),
            basic_auth_username: None,
        })
        .unwrap();
    store
        .save_key(SearchCap::Fetch, "jina", "one-key", None)
        .unwrap();
    store
        .save_key(SearchCap::Fetch, "jina", "two-key", None)
        .unwrap();
    assert_eq!(
        store.fetch("https://example.com").await.unwrap().content,
        "page body"
    );
    assert_eq!(
        store.fetch("https://example.com").await.unwrap_err().code(),
        KitErrorCode::Network
    );
    no_retry_rx.await.unwrap();
    // Cancel only once the request reached the server: a wall-clock budget also covers the
    // keyring read, which on a cold Linux Secret Service can outlast it before any byte is sent.
    tokio::select! {
        _ = store.fetch("https://example.com") => panic!("fetch finished instead of being cancelled"),
        _ = sent_rx => {}
    }
    server.await.unwrap();
}

#[test]
fn old_schema_is_rejected_without_reading_or_deleting_old_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("search.db");
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("CREATE TABLE search_settings(capability TEXT PRIMARY KEY,provider TEXT,max_results INTEGER); CREATE TABLE search_providers(capability TEXT,provider TEXT,api_key BLOB); PRAGMA user_version=1;").unwrap();
    drop(conn);
    assert!(Store::open(&file, AddressPolicy::default()).is_err());
    let conn = Connection::open(&file).unwrap();
    assert!(conn.prepare("SELECT api_key FROM search_providers").is_ok());
}

#[test]
fn rejected_preferences_preserve_policy_and_unreadable_keys_never_leak() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
    let good = SearchPreferences {
        model_tools_preferred: true,
        ..SearchPreferences::default()
    };
    store.save_preferences(good).unwrap();
    let invalid = SearchPreferences {
        blacklist: vec!["example.com", "bad host"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ..SearchPreferences::default()
    };
    assert!(store.save_preferences(invalid).is_err());
    assert!(store.preferences().unwrap().model_tools_preferred);
    assert!(store.preferences().unwrap().blacklist.is_empty());
    store.lock().unwrap().execute("INSERT INTO search_keys(id,capability,provider,key_ciphertext,position)VALUES('broken','search','exa',?1,0)", [b"not-a-valid-cipher".as_slice()]).unwrap();
    let settings = store.settings(SearchCap::Search).unwrap();
    let provider = settings.providers.iter().find(|p| p.id == "exa").unwrap();
    assert_eq!(provider.keys[0].masked, "[unreadable]");
    assert!(!provider.ready);
    assert!(store.reveal(SearchCap::Search, "exa", "broken").is_err());
    assert!(store.runtime(SearchCap::Search, Some("exa")).is_err());
    assert!(
        store
            .runtime(SearchCap::Search, Some("exa-mcp"))
            .unwrap()
            .1
            .is_empty()
    );
}

#[tokio::test]
async fn fetch_blocks_blacklisted_redirect_before_request_and_cuts_unicode_pages() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{Duration, timeout},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://{addr}/blocked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
    });
    let runtime = Runtime {
        provider: "fetch".into(),
        base: String::new(),
        key: None,
        user: None,
        max: 5,
    };
    let mut prefs = SearchPreferences {
        blacklist: vec![format!("http://{addr}/blocked")],
        ..SearchPreferences::default()
    };
    let error = protocol::fetch(
        &AddressPolicy::default(),
        &runtime,
        &format!("http://{addr}/start"),
        &prefs,
    )
    .await
    .unwrap_err();
    assert_eq!(error.details().reason(), Some("blacklisted"));
    server.await.unwrap();
    prefs.blacklist = vec!["EXAMPLE.COM.".into()];
    assert!(check_blacklist("https://a.example.com./page", &prefs).is_err());
    assert!(check_blacklist("https://notexample.com/page", &prefs).is_ok());
    prefs.compression = Compression::Cutoff;
    prefs.cutoff_limit = 101;
    let mut content = "🙂界".repeat(300);
    cutoff(&mut content, &prefs);
    assert_eq!(content.chars().count(), 101);
    assert!(content.ends_with('🙂'));
}
