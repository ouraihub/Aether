#![expect(clippy::unwrap_used, reason = "a failed assertion fails the test")]
use super::*;

    #[test]
    fn page_parsers_preserve_unicode_and_strip_executable_noise() {
        let expected = "正文🙂".repeat(500);
        for (provider, value) in [
            ("jina", json!({"data":{"title":"Page","content":expected}})),
            (
                "firecrawl",
                json!({
                    "data": {
                        "metadata": {"title": "Page"},
                        "markdown": expected,
                    },
                }),
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
        let parsed = html(
            "<html><head><title>Page</title><style>hidden</style></head>\
             <body>正文🙂<script>secret()</script>&amp; text</body></html>",
            "https://example.com",
        )
        .unwrap();
        assert_eq!(parsed.title, "Page");
        assert!(
            parsed.content.contains("正文🙂")
                && parsed.content.contains("& text")
        );
        assert!(
            !parsed.content.contains("secret()")
                && !parsed.content.contains("hidden")
        );
        assert_eq!(
            clip(&"🙂".repeat(2000), 1000).chars().count(),
            1000,
        );
    }

    #[tokio::test]
    async fn declared_oversized_body_is_rejected_before_download() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = stream.read(&mut request).await.unwrap();
            assert!(received > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3000000\r\n\
                     Connection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let result = crate::search::search(
            crate::execution::ResolvedConfig {
                provider: "searxng".into(),
                base_url: format!("http://{addr}"),
                api_keys: Vec::new(),
                basic_auth_username: None,
                preferences: crate::SearchPreferences::default(),
                address_policy: net::AddressPolicy::default(),
            },
            "query",
        )
        .await;
        assert_eq!(result.unwrap_err().message(), "response too large");
        server.await.unwrap();
    }