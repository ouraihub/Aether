use super::*;

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
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Status\\r\\nContent-Length: {}\\r\\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
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
        // Wait for cancellation to close the request, then ensure no next key
        // was sent.
        assert_eq!(stream.read(&mut bytes).await.unwrap(), 0);
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("search.db"),
        AddressPolicy::default(),
    )
    .unwrap();
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
    // Cancel only once the request reached the server: a wall-clock budget
    // also covers the keyring read, which on a cold Linux Secret Service can
    // outlast it before any byte is sent.
    tokio::select! {
        _ = store.fetch("https://example.com") => {
            panic!("fetch finished instead of being cancelled")
        },
        _ = sent_rx => {}
    }
    server.await.unwrap();
}
