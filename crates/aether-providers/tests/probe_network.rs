//! Network and cancellation behavior for `Store::probe`.
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_providers::{
    KitErrorCode, ProbeError, ProbeResult, ProbeTarget, Protocol, Store,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const KEY: &str = "sk-test-0123456789abcdefgh";
type Seen = Arc<tokio::sync::Mutex<Vec<String>>>;

async fn upstream(response: Option<Vec<u8>>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Seen::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut head = Vec::new();
            let mut buf = [0_u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            log.lock()
                .await
                .push(String::from_utf8_lossy(&head).to_lowercase());
            match &response {
                Some(bytes) => {
                    let _ = socket.write_all(bytes).await;
                    let _ = socket.shutdown().await;
                }
                None => tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    drop(socket);
                })
                .await
                .unwrap(),
            }
        }
    });
    (base, seen)
}

fn http(status: &str, body: &str) -> Option<Vec<u8>> {
    Some(
        format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
}

fn draft(base_url: &str, protocol: Protocol, endpoint_id: Option<&str>) -> ProbeTarget {
    ProbeTarget::Draft {
        base_url: base_url.to_owned(),
        protocol,
        endpoint_id: endpoint_id.map(str::to_owned),
        key_id: None,
    }
}

fn keyed(store: &Store, base_url: &str, keys: &[&str]) -> String {
    let saved = store
        .save(
            serde_json::from_value(serde_json::json!({ "name": "Acme", "endpoints": [{
                "slug": "main", "baseUrl": base_url, "protocol": "anthropic-messages", "models": [] }] }))
            .unwrap(),
        )
        .unwrap();
    let endpoint_id = saved[0].endpoints[0].id.clone();
    for key in keys {
        store
            .save_key(
                serde_json::from_value(
                    serde_json::json!({ "endpointId": endpoint_id, "key": key }),
                )
                .unwrap(),
            )
            .unwrap();
    }
    endpoint_id
}

async fn probe(store: &Store, target: ProbeTarget) -> ProbeResult {
    store
        .probe(target, &CancellationToken::new())
        .await
        .unwrap()
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn error(result: &ProbeResult) -> &aether_providers::ProbeError {
    assert!(result.models.is_empty());
    result.error.as_ref().unwrap()
}
