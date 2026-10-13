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
#[cfg(windows)]
#[test]
fn omission_clear_default_and_capability_isolation_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("search.db");
    let store = Store::open(&file, AddressPolicy::default()).unwrap();
    let body = |cap, provider: &str, key: Option<&str>| SearchSave {
        capability: cap,
        provider: provider.into(),
        base_url: None,
        api_key: key.map(ToOwned::to_owned),
        basic_auth_username: None,
    };
    store
        .save(body(SearchCap::Search, "tavily", Some("tvly-private-key")))
        .unwrap();
    store.save(body(SearchCap::Search, "tavily", None)).unwrap();
    assert_eq!(
        store.reveal(SearchCap::Search, "tavily").unwrap(),
        "tvly-private-key"
    );
    store
        .set_default(SearchCap::Search, "tavily", Some(9))
        .unwrap();
    store
        .set_default(SearchCap::Search, "exa-mcp", None)
        .unwrap();
    store.set_default(SearchCap::Fetch, "fetch", None).unwrap();
    assert_eq!(
        store.settings(SearchCap::Search).unwrap().max_results,
        Some(9)
    );
    assert!(store.reveal(SearchCap::Fetch, "tavily").is_err());
    drop(store);
    let store = Store::open(&file, AddressPolicy::default()).unwrap();
    assert_eq!(
        store.settings(SearchCap::Search).unwrap().max_results,
        Some(9)
    );
    assert_eq!(
        store.reveal(SearchCap::Search, "tavily").unwrap(),
        "tvly-private-key"
    );
    store
        .save(body(SearchCap::Search, "tavily", Some("")))
        .unwrap();
    assert!(store.reveal(SearchCap::Search, "tavily").is_err());
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
