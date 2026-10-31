//! Independent public search/fetch consumer behavior against controlled
//! loopback upstreams.
#![expect(clippy::unwrap_used, reason = "failed assertions fail the test")]

use aether_search::{
    execution::ResolvedConfig,
    fetch,
    search,
    Compression,
    SearchPreferences,
};
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
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
                    if let Some(end) = bytes
                        .windows(4)
                        .position(|part| part == b"\r\n\r\n") {
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
                // Early client cancellation and body-budget rejection may close
                // the socket.
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
            tokio::time::timeout(
                Duration::from_millis(150),
                self.requests.recv(),
            )
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
    json!({
        "results": [{"title":"First","url":"https://example.com/a","content":"body"}],
    })
    .to_string()
}

fn assert_error(
    error: aether_support::KitError,
    code: &str,
    field: Option<&str>,
    reason: &str,
) {
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(value["code"], code);
    if let Some(field) = field {
        assert_eq!(value["details"]["field"], field);
    }
    assert_eq!(value["details"]["reason"], reason);
}
mod standalone_cases;
