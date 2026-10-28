//! Independent public search/fetch consumer behavior against controlled loopback upstreams.
#![expect(clippy::unwrap_used, reason = "failed assertions fail the test")]

use aether_search::{Compression, SearchPreferences, execution::ResolvedConfig, fetch, search};
use aether_support::net::{AddressPolicy, HostPort};
use serde_json::json;
use std::{collections::VecDeque, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};
use zeroize::Zeroizing;

struct Reply {
    wire: Vec<u8>,
    delay: Duration,
}

impl Reply {
    fn status(status: u16, body: &str) -> Self {
        Self::raw(format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
    }

    fn raw(wire: String) -> Self {
        Self {
            wire: wire.into_bytes(),
            delay: Duration::ZERO,
        }
    }
}

struct Server {
    url: String,
    port: u16,
    requests: mpsc::UnboundedReceiver<String>,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut replies = VecDeque::from(replies);
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                if sender.send(String::from_utf8(bytes).unwrap()).is_err() {
                    break;
                }
                let reply = replies
                    .pop_front()
                    .unwrap_or_else(|| Reply::status(500, "unexpected request"));
                tokio::time::sleep(reply.delay).await;
                // Early client cancellation and body-budget rejection may close the socket.
                let _ = stream.write_all(&reply.wire).await;
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            port,
            requests,
            task,
        }
    }

    async fn request(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(2), self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn no_request(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(150), self.requests.recv())
                .await
                .is_err()
        );
    }

    fn config(&self, provider: &str, keys: &[&str]) -> ResolvedConfig {
        ResolvedConfig {
            provider: provider.into(),
            base_url: self.url.clone(),
            api_keys: keys
                .iter()
                .map(|key| Zeroizing::new((*key).to_owned()))
                .collect(),
            basic_auth_username: None,
            preferences: SearchPreferences::default(),
            address_policy: AddressPolicy {
                allow_private: vec![HostPort {
                    host: "127.0.0.1".into(),
                    port: self.port,
                }],
            },
        }
    }
}

fn results() -> String {
    json!({"results":[{"title":"First","url":"https://example.com/a","content":"body"}]})
        .to_string()
}

fn assert_error(error: aether_support::KitError, code: &str, field: Option<&str>, reason: &str) {
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(value["code"], code);
    if let Some(field) = field {
        assert_eq!(value["details"]["field"], field);
    }
    assert_eq!(value["details"]["reason"], reason);
}

#[tokio::test]
async fn explicit_keys_authenticate_in_given_order_only_on_401_403_429() {
    for status in [401, 403, 429] {
        let mut server = Server::start(vec![
            Reply::status(status, "denied"),
            Reply::status(200, &results()),
        ])
        .await;
        let result = search::search(
            server.config("tavily", &["fixture-first", "fixture-second"]),
            "  世界 query  ",
        )
        .await
        .unwrap();
        assert_eq!(result.query, "世界 query");
        assert_eq!(result.provider, "tavily");
        assert_eq!(result.hits[0].snippet, "body");
        for key in ["fixture-first", "fixture-second"] {
            let request = server.request().await;
            assert!(request.starts_with("POST /search HTTP/1.1\r\n"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {key}\r\n"))
            );
            let body = request.split_once("\r\n\r\n").unwrap().1;
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(body).unwrap()["query"],
                "世界 query"
            );
        }
        server.no_request().await;
        // A fresh explicit snapshot must start at its first key, not a global cursor.
        assert!(
            search::search(
                server.config("tavily", &["fixture-first", "fixture-second"]),
                "again"
            )
            .await
            .is_err()
        );
        assert!(
            server
                .request()
                .await
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-first\r\n")
        );
        server.no_request().await;
    }
}

#[tokio::test]
async fn server_errors_do_not_retry_or_fall_back_to_anonymous() {
    for status in [500, 503] {
        let mut server = Server::start(vec![Reply::status(status, "failed")]).await;
        assert!(
            search::search(
                server.config("tavily", &["fixture-first", "fixture-second"]),
                "query"
            )
            .await
            .is_err()
        );
        server.request().await;
        server.no_request().await;
    }
    let mut server = Server::start(vec![
        Reply::status(401, "denied"),
        Reply::status(403, "denied"),
    ])
    .await;
    assert!(
        search::search(
            server.config("tavily", &["fixture-first", "fixture-second"]),
            "query"
        )
        .await
        .is_err()
    );
    server.request().await;
    server.request().await;
    server.no_request().await;
}

#[tokio::test]
async fn parsing_deduplication_blacklist_and_unicode_cutoff_share_public_path() {
    let text = "界🙂".repeat(80);
    let body = json!({"results":[
        {"title":"blocked","url":"https://sub.blocked.example/a","content":"blocked"},
        {"title":"invalid","url":"file:///private","content":"invalid"},
        {"title":"first","url":"https://example.com/a","content":text},
        {"title":"duplicate","url":"https://example.com/a","content":"duplicate"},
        {"title":"second","link":"https://example.com/b","content":"second"},
        {"title":"third","url":"https://example.com/c","content":"third"}
    ]})
    .to_string();
    let mut server = Server::start(vec![Reply::status(200, &body)]).await;
    let mut config = server.config("tavily", &["fixture-key"]);
    config.preferences.max_results = 2;
    config.preferences.blacklist = vec!["blocked.example".into()];
    config.preferences.compression = Compression::Cutoff;
    config.preferences.cutoff_limit = 100;
    let result = search::search(config, "query").await.unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.hits[0].title, "first");
    assert_eq!(result.hits[0].snippet, "界🙂".repeat(50));
    assert_eq!(result.hits[1].url, "https://example.com/b");
    server.request().await;
}

#[tokio::test]
async fn direct_fetch_has_no_provider_credentials_and_returns_a_page() {
    let mut server = Server::start(vec![Reply::status(200, "<html><title>Local title</title><style>hidden</style><script>secret()</script><p>Readable 世界</p></html>")]).await;
    let mut config = server.config("fetch", &["fixture-provider-secret"]);
    config.basic_auth_username = Some("fixture-user".into());
    config.base_url = "ignored direct-fetch base".into();
    let page = fetch::fetch(config, &format!("{}/document", server.url))
        .await
        .unwrap();
    assert_eq!(page.title, "Local title");
    assert!(page.content.contains("Readable 世界"));
    assert!(!page.content.contains("secret()"));
    assert!(!page.content.contains("hidden"));
    let request = server.request().await.to_ascii_lowercase();
    assert!(request.starts_with("get /document http/1.1"));
    for forbidden in [
        "authorization:",
        "x-api-key:",
        "x-subscription-token:",
        "fixture-provider-secret",
        "fixture-user",
    ] {
        assert!(!request.contains(forbidden));
    }
}

#[tokio::test]
async fn redirect_blacklist_stops_before_target_request() {
    let mut target = Server::start(vec![Reply::status(200, "must not be read")]).await;
    let mut origin = Server::start(vec![Reply::raw(format!("HTTP/1.1 302 Found\r\nLocation: {}/blocked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", target.url))]).await;
    let mut config = origin.config("fetch", &[]);
    config.address_policy.allow_private.push(HostPort {
        host: "127.0.0.1".into(),
        port: target.port,
    });
    config
        .preferences
        .blacklist
        .push(format!("{}/blocked", target.url));
    assert!(
        fetch::fetch(config, &format!("{}/start", origin.url))
            .await
            .is_err()
    );
    origin.request().await;
    target.no_request().await;
}

#[tokio::test]
async fn public_inputs_reject_invalid_provider_keys_preferences_query_and_url_without_http() {
    let mut server = Server::start(vec![]).await;
    for provider in ["unknown", "fetch"] {
        assert_error(
            search::search(server.config(provider, &[]), "query")
                .await
                .unwrap_err(),
            "bad_request",
            Some("provider"),
            "unsupported",
        );
    }
    assert_error(
        fetch::fetch(server.config("tavily", &["fixture-key"]), &server.url)
            .await
            .unwrap_err(),
        "bad_request",
        Some("provider"),
        "unsupported",
    );
    assert_error(
        search::search(server.config("tavily", &[]), "query")
            .await
            .unwrap_err(),
        "needs_config",
        None,
        "keys",
    );
    for query in [" \n\t".to_owned(), "界".repeat(4097)] {
        assert_error(
            search::search(server.config("tavily", &["fixture-key"]), &query)
                .await
                .unwrap_err(),
            "bad_request",
            Some("query"),
            "invalid",
        );
    }
    for prefs in [
        SearchPreferences {
            max_results: 0,
            ..SearchPreferences::default()
        },
        SearchPreferences {
            cutoff_limit: 99,
            ..SearchPreferences::default()
        },
        SearchPreferences {
            blacklist: vec!["[invalid".into()],
            ..SearchPreferences::default()
        },
    ] {
        let mut config = server.config("tavily", &["fixture-key"]);
        config.preferences = prefs.clone();
        assert!(search::search(config, "query").await.is_err());
        let mut config = server.config("fetch", &[]);
        config.preferences = prefs;
        assert!(fetch::fetch(config, &server.url).await.is_err());
    }
    assert_error(
        fetch::fetch(server.config("fetch", &[]), &"x".repeat(8193))
            .await
            .unwrap_err(),
        "bad_request",
        Some("url"),
        "too_long",
    );
    for url in ["file:///private", "not a URL", "ftp://example.com/a"] {
        assert!(
            fetch::fetch(server.config("fetch", &[]), url)
                .await
                .is_err()
        );
    }
    server.no_request().await;
}

#[tokio::test]
async fn dropping_pending_search_prevents_subsequent_key_attempts() {
    let mut denied = Reply::status(401, "denied");
    denied.delay = Duration::from_millis(75);
    let mut server = Server::start(vec![denied, Reply::status(200, &results())]).await;
    let config = server.config("tavily", &["fixture-first", "fixture-second"]);
    let mut pending = Box::pin(search::search(config, "query"));
    tokio::select! {
        result = &mut pending => panic!("request completed before cancellation: {result:?}"),
        request = server.request() => assert!(request.contains("fixture-first")),
    }
    drop(pending);
    server.no_request().await;
}

#[tokio::test]
async fn declared_and_chunked_oversize_bodies_are_rejected_without_key_retry() {
    let oversized = "x".repeat(2 * 1024 * 1024 + 1);
    let replies = [
        Reply::raw(
            "HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\nConnection: close\r\n\r\n".into(),
        ),
        Reply::raw(format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n",
            oversized.len()
        )),
    ];
    for reply in replies {
        let mut server = Server::start(vec![reply]).await;
        assert!(
            tokio::time::timeout(
                Duration::from_secs(2),
                search::search(
                    server.config("tavily", &["fixture-first", "fixture-second"]),
                    "query"
                )
            )
            .await
            .unwrap()
            .is_err()
        );
        server.request().await;
        server.no_request().await;
    }
}

#[tokio::test]
async fn network_failure_does_not_attempt_the_second_key() {
    let mut server = Server::start(vec![
        Reply::raw(String::new()),
        Reply::status(200, &results()),
    ])
    .await;
    assert!(
        search::search(
            server.config("tavily", &["fixture-first", "fixture-second"]),
            "query"
        )
        .await
        .is_err()
    );
    server.request().await;
    server.no_request().await;
}

#[tokio::test]
async fn maximum_unicode_query_and_anonymous_search_are_accepted() {
    let mut server = Server::start(vec![Reply::status(200, &results())]).await;
    let query = "界".repeat(4096);
    let result = search::search(server.config("searxng", &[]), &query)
        .await
        .unwrap();
    assert_eq!(result.query, query);
    let request = server.request().await.to_ascii_lowercase();
    assert!(request.starts_with("get /search?"));
    assert!(!request.contains("authorization:"));
    server.no_request().await;
}

#[tokio::test]
async fn direct_fetch_enforces_chunked_body_budget_and_unicode_page_safety_limit() {
    let oversized = "x".repeat(2 * 1024 * 1024 + 1);
    let reply = Reply::raw(format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n",
        oversized.len()
    ));
    let mut server = Server::start(vec![reply]).await;
    assert!(
        fetch::fetch(server.config("fetch", &[]), &server.url)
            .await
            .is_err()
    );
    server.request().await;
    let body = format!("<title>Unicode</title><p>{}</p>", "界🙂".repeat(13000));
    let mut server = Server::start(vec![Reply::status(200, &body)]).await;
    let page = fetch::fetch(server.config("fetch", &[]), &server.url)
        .await
        .unwrap();
    assert!(page.content.chars().count() <= 24000);
    assert!(page.content.contains("界🙂"));
    server.request().await;
}

#[tokio::test]
async fn redirect_to_link_local_address_is_rejected_by_address_policy() {
    let mut origin = Server::start(vec![Reply::raw("HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/blocked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into())]).await;
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        fetch::fetch(origin.config("fetch", &[]), &origin.url),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(serde_json::to_value(error).unwrap()["code"], "bad_request");
    origin.request().await;
    origin.no_request().await;
}

#[tokio::test]
async fn supplier_fetch_rejects_target_before_http_and_rechecks_returned_source() {
    let mut server = Server::start(vec![]).await;
    let mut config = server.config("firecrawl", &[]);
    config.preferences.blacklist.push("blocked.example".into());
    assert_error(
        fetch::fetch(config, "https://blocked.example/page")
            .await
            .unwrap_err(),
        "bad_request",
        Some("url"),
        "blacklisted",
    );
    assert!(
        fetch::fetch(
            server.config("firecrawl", &[]),
            "http://169.254.169.254/private"
        )
        .await
        .is_err()
    );
    server.no_request().await;
    let body = json!({"success":true,"data":{"markdown":"content","metadata":{"title":"Page","sourceURL":"https://blocked.example/returned"}}}).to_string();
    let mut server = Server::start(vec![Reply::status(200, &body)]).await;
    let mut config = server.config("firecrawl", &[]);
    config.preferences.blacklist.push("blocked.example".into());
    assert_error(
        fetch::fetch(config, "https://example.com/page")
            .await
            .unwrap_err(),
        "bad_request",
        Some("url"),
        "blacklisted",
    );
    server.request().await;
}

async fn total_deadline(provider: &str, fetch_page: bool, budget: u64) {
    let mut denied = Reply::status(401, "denied");
    denied.delay = Duration::from_secs(budget / 2 + 1);
    let mut success = Reply::status(200, &results());
    success.delay = denied.delay;
    let mut server = Server::start(vec![denied, success]).await;
    let config = server.config(provider, &["fixture-first", "fixture-second"]);
    let started = tokio::time::Instant::now();
    let failed = tokio::time::timeout(Duration::from_secs(budget + 3), async {
        if fetch_page {
            fetch::fetch(config, "https://example.com/page")
                .await
                .is_err()
        } else {
            search::search(config, "query").await.is_err()
        }
    })
    .await
    .unwrap();
    assert!(failed);
    assert!(started.elapsed() >= Duration::from_secs(budget - 1));
    assert!(started.elapsed() < Duration::from_secs(budget + 3));
    for key in ["fixture-first", "fixture-second"] {
        assert!(server.request().await.contains(key));
    }
    server.no_request().await;
}

#[tokio::test]
async fn total_deadlines_cover_all_key_attempts_for_search_mcp_and_fetch() {
    tokio::join!(
        total_deadline("tavily", false, 15),
        total_deadline("exa-mcp", false, 25),
        total_deadline("firecrawl", true, 20),
    );
}
