#![expect(clippy::unwrap_used, reason = "failed assertions fail the test")]
use super::*;
use crate::{
    Compression, SearchHit,
    execution::ResolvedConfig,
    policy::{check_blacklist, cutoff, process_hits},
    vendor,
};
use aether_testkit::{Exhausted, Gate, Reply, Server};
use serde_json::json;
use std::time::Duration;
use zeroize::Zeroizing;

/// A request past the script fails loudly instead of hanging.
fn unexpected() -> Exhausted {
    Exhausted::Reply(Reply::status(500, "unexpected request"))
}

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
        assert!(vendor::parse_hits(provider, &json!({"error":"bad"}), 5).is_err());
    }
    let body = "data: {\"result\":{\"content\":[{\"text\":\"Title: A\\nURL: https://example.com\\nText: line one\\nline two\"}]}}\n\n";
    assert_eq!(
        vendor::parse_mcp(body, 5).unwrap()[0].snippet,
        "line one\nline two"
    );
    assert!(vendor::parse_mcp("{\"error\":{\"message\":\"bad\"}}", 5).is_err());
}
#[test]
fn mcp_highlights_keep_paragraphs_between_results() {
    let body = json!({"result":{"content":[{"text":"Title: First\nURL: https://example.com/first\nHighlights:\nIntro\n\n# Body\nDetails\n\nTitle: Second\nURL: https://example.com/second\nText: Last"}]}}).to_string();
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
    let body = "{\"results\":[{\"title\":\"A\",\"url\":\"https://example.com\",\"content\":\"body\"},{\"title\":\"B\",\"url\":\"https://other.com\",\"content\":\"other\"}]}";
    let mut server = Server::script(vec![Reply::json(body)], unexpected()).await;
    let config = ResolvedConfig {
        provider: "searxng".into(),
        base_url: server.url(),
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
    let request = server.request().await;
    assert_eq!(
        request.header("authorization"),
        Some("Basic dXNlcjpwYXNzd29yZA==")
    );
    assert_eq!(request.method, "GET");
    assert!(request.target.starts_with("/search?"));
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
    let parsed = vendor::parse_hits(
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
    let hits =
        "{\"results\":[{\"title\":\"A\",\"url\":\"https://example.com\",\"text\":\"body\"}]}";
    let server = Server::script(
        [401, 200, 200, 500, 429, 403]
            .into_iter()
            .map(|status| match status {
                200 => Reply::json(hits),
                _ => Reply::status(status, "rejected"),
            })
            .collect(),
        unexpected(),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
    store
        .save(SearchSave {
            capability: SearchCap::Search,
            provider: "exa".into(),
            base_url: Some(server.url()),
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
    let requests = server.seen();
    assert_eq!(requests.len(), 6);
    for (request, key) in requests.iter().zip([
        "first-key",
        "second-key",
        "second-key",
        "first-key",
        "second-key",
        "first-key",
    ]) {
        assert_eq!(request.header("x-api-key"), Some(key));
    }
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn fetch_retries_keys_but_network_and_cancel_do_not() {
    use tokio::{io::AsyncReadExt, net::TcpListener, time::timeout};
    let body = "{\"data\":{\"content\":\"page body\"}}";
    let mut keyed = Server::script(
        vec![
            Reply::status(403, body),
            Reply::json(body),
            // Hang up without a byte: a network failure must not move on to the next key.
            Reply::close(),
        ],
        unexpected(),
    )
    .await;
    // Cancellation is observed as the client closing its socket, so this last upstream stays
    // bespoke: it reads the request, then waits for EOF and checks no next key is sent.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
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
            base_url: Some(keyed.url()),
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
    for key in ["one-key", "two-key"] {
        assert_eq!(
            keyed.request().await.header("authorization"),
            Some(format!("Bearer {key}").as_str())
        );
    }
    assert_eq!(
        store.fetch("https://example.com").await.unwrap_err().code(),
        KitErrorCode::Network
    );
    keyed.request().await;
    keyed.no_request(Duration::from_millis(150)).await;
    store
        .save(SearchSave {
            capability: SearchCap::Fetch,
            provider: "jina".into(),
            base_url: Some(format!("http://{addr}")),
            basic_auth_username: None,
        })
        .unwrap();
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
    assert!(store.resolve(SearchCap::Search, Some("exa")).is_err());
    assert!(
        store
            .resolve(SearchCap::Search, Some("exa-mcp"))
            .unwrap()
            .api_keys
            .is_empty()
    );
}

#[tokio::test]
async fn fetch_blocks_blacklisted_redirect_before_request_and_cuts_unicode_pages() {
    // The redirect points back at this server, whose address is only known once it is bound.
    let mut server = Server::handler(|request| {
        let host = request.header("host").unwrap_or_default();
        Reply::http(302, &[("location", &format!("http://{host}/blocked"))], "")
    })
    .await;
    let addr = format!("127.0.0.1:{}", server.port());
    let mut prefs = SearchPreferences {
        blacklist: vec![format!("http://{addr}/blocked")],
        ..SearchPreferences::default()
    };
    let error = crate::fetch::fetch(
        ResolvedConfig {
            provider: "fetch".into(),
            base_url: String::new(),
            api_keys: Vec::new(),
            basic_auth_username: None,
            preferences: prefs.clone(),
            address_policy: AddressPolicy::default(),
        },
        &format!("http://{addr}/start"),
    )
    .await
    .unwrap_err();
    assert_eq!(error.details().reason(), Some("blacklisted"));
    assert_eq!(server.request().await.target, "/start");
    server.no_request(Duration::from_millis(150)).await;
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

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[test]
fn resolve_skips_unreadable_keys_rotates_partitions_and_freezes_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
    for (cap, values) in [
        (SearchCap::Search, ["search-one", "search-two"]),
        (SearchCap::Fetch, ["fetch-one", "fetch-two"]),
    ] {
        for value in values {
            store.save_key(cap, "querit", value, None).unwrap();
        }
    }
    store.lock().unwrap().execute("INSERT INTO search_keys(id,capability,provider,key_ciphertext,position)VALUES('broken','search','querit',?1,-1)", [b"invalid-ciphertext".as_slice()]).unwrap();
    let snapshot = store.resolve(SearchCap::Search, Some("querit")).unwrap();
    assert_eq!(
        snapshot
            .api_keys
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>(),
        ["search-one", "search-two"]
    );
    assert_eq!(
        store
            .resolve(SearchCap::Search, Some("querit"))
            .unwrap()
            .api_keys[0]
            .as_str(),
        "search-two"
    );
    assert_eq!(
        store
            .resolve(SearchCap::Fetch, Some("querit"))
            .unwrap()
            .api_keys[0]
            .as_str(),
        "fetch-one"
    );
    assert_eq!(
        store
            .resolve(SearchCap::Search, Some("querit"))
            .unwrap()
            .api_keys[0]
            .as_str(),
        "search-one"
    );
    store
        .save(SearchSave {
            capability: SearchCap::Search,
            provider: "querit".into(),
            base_url: Some("http://localhost:8088".into()),
            basic_auth_username: None,
        })
        .unwrap();
    let id = store
        .settings(SearchCap::Search)
        .unwrap()
        .providers
        .into_iter()
        .find(|provider| provider.id == "querit")
        .unwrap()
        .keys
        .into_iter()
        .find(|key| key.id != "broken")
        .unwrap()
        .id;
    store.remove_key(SearchCap::Search, "querit", &id).unwrap();
    store
        .save_preferences(SearchPreferences {
            max_results: 1,
            ..SearchPreferences::default()
        })
        .unwrap();
    store.set_default(SearchCap::Search, "querit").unwrap();
    assert_eq!(snapshot.base_url, "https://api.querit.ai");
    assert_eq!(snapshot.preferences.max_results, 5);
    assert_eq!(snapshot.api_keys[0].as_str(), "search-one");
    let next = store.resolve(SearchCap::Search, None).unwrap();
    assert_eq!(next.provider, "querit");
    assert_eq!(next.base_url, "http://localhost:8088");
    assert_eq!(next.preferences.max_results, 1);
    assert_eq!(next.api_keys.len(), 1);
    assert_eq!(
        store.settings(SearchCap::Fetch).unwrap().default_provider,
        "jina"
    );
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[test]
fn failed_key_write_preserves_previous_ciphertext_and_rolls_back_new_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
    let settings = store
        .save_key(SearchCap::Search, "exa", "old-fixture", None)
        .unwrap();
    let id = &settings
        .providers
        .iter()
        .find(|provider| provider.id == "exa")
        .unwrap()
        .keys[0]
        .id;
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_key_update BEFORE UPDATE ON search_keys BEGIN SELECT RAISE(ABORT,'fixture write failure'); END; CREATE TRIGGER reject_key_insert BEFORE INSERT ON search_keys BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
    assert!(
        store
            .save_key(SearchCap::Search, "exa", "replacement-fixture", Some(id))
            .is_err()
    );
    assert!(
        store
            .save_key(SearchCap::Search, "exa", "new-fixture", None)
            .is_err()
    );
    assert_eq!(
        store.reveal(SearchCap::Search, "exa", id).unwrap(),
        "old-fixture"
    );
    assert_eq!(
        Store::keys(&store.lock().unwrap(), SearchCap::Search, "exa")
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn check_uses_selected_provider_and_canonical_inputs_without_holding_store_lock() {
    let body = "{\"results\":[]}";
    let release = Gate::new();
    let mut server = Server::script(
        vec![
            Reply::close().then_wait(&release).then_raw(format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )),
            Reply::json("{\"data\":{\"content\":\"readable\"}}"),
        ],
        unexpected(),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.db"), AddressPolicy::default()).unwrap();
    for (cap, provider) in [(SearchCap::Search, "searxng"), (SearchCap::Fetch, "jina")] {
        store
            .save(SearchSave {
                capability: cap,
                provider: provider.into(),
                base_url: Some(server.url()),
                basic_auth_username: None,
            })
            .unwrap();
    }
    let check = store.check(SearchCap::Search, "searxng");
    tokio::pin!(check);
    tokio::select! {
        _ = &mut check => panic!("check finished before fixture response"),
        request = server.request() => assert_eq!(
            (request.method.as_str(), request.target.as_str()),
            ("GET", "/search?q=web+search&format=json&language=auto")
        ),
    }
    assert!(store.db.try_lock().is_ok());
    store
        .save_preferences(SearchPreferences {
            max_results: 1,
            ..SearchPreferences::default()
        })
        .unwrap();
    release.open();
    let error = check.await.unwrap_err();
    assert_eq!(error.code(), KitErrorCode::Upstream);
    assert_eq!(error.message(), "provider returned no results");
    assert_eq!(
        store.check(SearchCap::Fetch, "jina").await.unwrap().count,
        1
    );
    let fetched = server.request().await;
    assert_eq!(
        (fetched.method.as_str(), fetched.target.as_str()),
        ("GET", "/https://example.com")
    );
    assert_eq!(
        store.settings(SearchCap::Search).unwrap().default_provider,
        "exa-mcp"
    );
    server.no_request(Duration::from_millis(150)).await;
}
