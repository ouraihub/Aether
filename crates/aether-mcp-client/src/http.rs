//! Streamable HTTP transport (docs/27 §4). Hand-written on `aether_support::net` so every
//! request goes through the address policy and the no-redirect client.

use std::time::Duration;

use aether_support::net::{self, AddressPolicy, HostPort};
use aether_support::sse::{SseEvent, SseParser};
use aether_support::{KitError, KitErrorCode};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Response, StatusCode};
use serde_json::Value;
use url::{Host, Url};

use crate::credentials::Secret;
use crate::protocol::{MAX_MESSAGE_BYTES, RpcError, Transport, find, invalid_response, too_large};

const SESSION_HEADER: &str = "mcp-session-id";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The policy for one server: its own `host:port` is allowed when the user granted it.
pub(crate) fn policy(url: &str, allow_private: bool) -> AddressPolicy {
    let target = Url::parse(url)
        .ok()
        .filter(|_| allow_private)
        .and_then(|url| {
            let host = match url.host()? {
                Host::Domain(domain) => domain.to_owned(),
                Host::Ipv4(ip) => ip.to_string(),
                Host::Ipv6(ip) => ip.to_string(),
            };
            Some(HostPort {
                host,
                port: url.port_or_known_default()?,
            })
        });
    AddressPolicy {
        allow_private: target.into_iter().collect(),
    }
}

/// One http server: credentials applied, session id remembered.
pub(crate) struct HttpLink {
    http: reqwest::Client,
    /// The stored url plus the secret query parameters; never logged.
    url: Url,
    headers: HeaderMap,
    known: Vec<String>,
    session: Option<HeaderValue>,
}

impl HttpLink {
    /// Validates the url against the policy; nothing is sent yet.
    pub(crate) fn new(url: &str, secret: &Secret, allow_private: bool) -> Result<Self, KitError> {
        let policy = policy(url, allow_private);
        let mut full = net::check_url(url, &policy)?;
        if !secret.query.is_empty() {
            full.query_pairs_mut().extend_pairs(&secret.query);
        }
        Ok(Self {
            http: net::client(policy, CONNECT_TIMEOUT)?,
            url: full,
            headers: secret.header_map()?,
            known: secret.values(),
            session: None,
        })
    }

    async fn post(&self, body: &Value) -> Result<Response, RpcError> {
        let mut request = self
            .http
            .post(self.url.clone())
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string());
        if let Some(session) = &self.session {
            request = request.header(SESSION_HEADER, session);
        }
        request.send().await.map_err(|error| {
            let error = error.without_url();
            if net::is_blocked(&error) {
                RpcError::Failed(KitError::bad_request("url", "blocked_address").with_cause(error))
            } else if error.is_connect() {
                RpcError::Failed(unreachable(error))
            } else {
                RpcError::Unknown(unreachable(error))
            }
        })
    }

    /// JSON body or SSE stream; SSE yields the first event whose message has our `id`.
    async fn read_response(&self, mut response: Response, id: u64) -> Result<Value, RpcError> {
        let sse = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        let mut parser = SseParser::new();
        let mut body = Vec::new();
        let mut total = 0_usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| RpcError::Unknown(unreachable(error.without_url())))?
        {
            total += chunk.len();
            if total > MAX_MESSAGE_BYTES {
                return Err(RpcError::Unknown(too_large()));
            }
            if sse {
                if let Some(message) = find_in_events(parser.feed(&chunk), id) {
                    return Ok(message);
                }
            } else {
                body.extend_from_slice(&chunk);
            }
        }
        if sse {
            find_in_events(parser.finish(), id).ok_or_else(|| {
                RpcError::Unknown(KitError::new(
                    KitErrorCode::Network,
                    "event stream ended without a response",
                ))
            })
        } else {
            serde_json::from_slice(&body)
                .ok()
                .and_then(|message| find(message, id))
                .ok_or_else(|| RpcError::Unknown(invalid_response()))
        }
    }

    async fn upstream(&self, mut response: Response) -> KitError {
        let status = response.status().as_u16();
        let mut body = Vec::new();
        // The excerpt is best effort: a broken error body still yields the status.
        while let Ok(Some(chunk)) = response.chunk().await {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_MESSAGE_BYTES {
                break;
            }
        }
        let known: Vec<&str> = self.known.iter().map(String::as_str).collect();
        KitError::upstream(status, &String::from_utf8_lossy(&body), &known)
    }
}

impl Transport for HttpLink {
    async fn request(&mut self, message: &Value, id: u64) -> Result<Value, RpcError> {
        let had_session = self.session.is_some();
        let response = self.post(message).await?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND && had_session {
            return Err(RpcError::Expired);
        }
        if !status.is_success() {
            return Err(RpcError::Failed(self.upstream(response).await));
        }
        if let Some(session) = response.headers().get(SESSION_HEADER) {
            self.session = Some(session.clone());
        }
        self.read_response(response, id).await
    }

    async fn notify(&mut self, message: &Value) -> Result<(), RpcError> {
        let response = self.post(message).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(RpcError::Failed(self.upstream(response).await))
        }
    }

    fn reset(&mut self) {
        self.session = None;
    }

    /// Each request is its own HTTP exchange; a dropped one leaves the link usable.
    fn abandon(&mut self) {}

    fn is_alive(&self) -> bool {
        true
    }

    fn known(&self) -> &[String] {
        &self.known
    }
}

fn unreachable(error: reqwest::Error) -> KitError {
    let code = if error.is_timeout() {
        KitErrorCode::Timeout
    } else {
        KitErrorCode::Network
    };
    KitError::new(code, "MCP server unreachable").with_cause(error)
}

fn find_in_events(events: Vec<SseEvent>, id: u64) -> Option<Value> {
    events.into_iter().find_map(|event| {
        serde_json::from_str(&event.data)
            .ok()
            .and_then(|message| find(message, id))
    })
}
