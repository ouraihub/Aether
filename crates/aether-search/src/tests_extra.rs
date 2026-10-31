#[test]
fn old_schema_is_rejected_without_reading_or_deleting_old_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("search.db");
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch(
        "CREATE TABLE search_settings(capability TEXT PRIMARY KEY,provider TEXT,\
         max_results INTEGER); CREATE TABLE search_providers(capability TEXT,\
         provider TEXT,api_key BLOB); PRAGMA user_version=1;",
    )
    .unwrap();
    drop(conn);
    assert!(Store::open(&file, AddressPolicy::default()).is_err());
    let conn = Connection::open(&file).unwrap();
    assert!(conn.prepare("SELECT api_key FROM search_providers").is_ok());
}

#[test]
fn rejected_preferences_preserve_policy_and_unreadable_keys_never_leak() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("search.db"),
        AddressPolicy::default(),
    )
    .unwrap();
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
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO search_keys(id,capability,provider,key_ciphertext,\
             position) VALUES('broken','search','exa',?1,0)",
            [b"not-a-valid-cipher".as_slice()],
        )
        .unwrap();
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
async fn fetch_blocks_blacklisted_redirect_before_request_and_cuts_unicode_pages(
)
{
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
        stream
            .write_all(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{addr}/blocked\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
    });
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

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[test]
fn resolve_skips_unreadable_keys_rotates_partitions_and_freezes_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("search.db"),
        AddressPolicy::default(),
    )
    .unwrap();
    for (cap, values) in [
        (SearchCap::Search, ["search-one", "search-two"]),
        (SearchCap::Fetch, ["fetch-one", "fetch-two"]),
    ] {
        for value in values {
            store.save_key(cap, "querit", value, None).unwrap();
        }
    }
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO search_keys(id,capability,provider,key_ciphertext,\
             position) VALUES('broken','search','querit',?1,-1)",
            [b"invalid-ciphertext".as_slice()],
        )
        .unwrap();
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
    let store = Store::open(
        &dir.path().join("search.db"),
        AddressPolicy::default(),
    )
    .unwrap();
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
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_key_update BEFORE UPDATE ON search_keys BEGIN \
             SELECT RAISE(ABORT,'fixture write failure'); END; CREATE TRIGGER \
             reject_key_insert BEFORE INSERT ON search_keys BEGIN SELECT \
             RAISE(ABORT,'fixture write failure'); END;",
        )
        .unwrap();
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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (sent, received) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        let size = stream.read(&mut bytes).await.unwrap();
        assert!(
            String::from_utf8_lossy(&bytes[..size])
                .starts_with(
                    "GET /search?q=web+search&format=json&language=auto "
                )
        );
        sent.send(()).unwrap();
        released.await.unwrap();
        let body = "{\"results\":[]}";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let size = stream.read(&mut bytes).await.unwrap();
        assert!(
            String::from_utf8_lossy(&bytes[..size])
                .starts_with("GET /https://example.com ")
        );
        let body = "{\"data\":{\"content\":\"readable\"}}";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("search.db"),
        AddressPolicy::default(),
    )
    .unwrap();
    for (cap, provider) in [
        (SearchCap::Search, "searxng"),
        (SearchCap::Fetch, "jina"),
    ] {
        store
            .save(SearchSave {
                capability: cap,
                provider: provider.into(),
                base_url: Some(format!("http://{addr}")),
                basic_auth_username: None,
            })
            .unwrap();
    }
    let check = store.check(SearchCap::Search, "searxng");
    tokio::pin!(check);
    tokio::select! {
        _ = &mut check => panic!("check finished before fixture response"),
        _ = received => {}
    }
    assert!(store.db.try_lock().is_ok());
    store
        .save_preferences(SearchPreferences {
            max_results: 1,
            ..SearchPreferences::default()
        })
        .unwrap();
    release.send(()).unwrap();
    let error = check.await.unwrap_err();
    assert_eq!(error.code(), KitErrorCode::Upstream);
    assert_eq!(error.message(), "provider returned no results");
    assert_eq!(
        store.check(SearchCap::Fetch, "jina").await.unwrap().count,
        1
    );
    assert_eq!(
        store.settings(SearchCap::Search).unwrap().default_provider,
        "exa-mcp"
    );
    server.await.unwrap();
}