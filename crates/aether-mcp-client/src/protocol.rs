//! Streamable HTTP JSON-RPC client (docs/13 §3). Hand-written on `aether_support::net`
//! so every request goes through the address policy and the no-redirect client.

use std::collections::BTreeMap;
use std::time::Duration;

use aether_support::net::{self, AddressPolicy, HostPort};
use aether_support::sse::{SseEvent, SseParser};
use aether_support::{KitError, KitErrorCode};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

const PROTOCOL_VERSION: &str = "2025-03-26";
const ACCEPTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26"];
const SESSION_HEADER: &str = "mcp-session-id";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per request, from connecting until the body is read.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 10;
const MAX_TOOLS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_SCHEMA_BYTES: usize = 32 * 1024;

/// Credentials of one server, sealed at rest as this JSON (docs/13 §2).
// ponytail: the parsed maps are plain Strings, not zeroized; only the sealed JSON is.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Secret {
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) query: BTreeMap<String, String>,
}

impl Secret {
    pub(crate) fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.query.is_empty()
    }

    /// Names only, for display.
    pub(crate) fn keys(&self) -> Vec<String> {
        self.headers
            .keys()
            .chain(self.query.keys())
            .cloned()
            .collect()
    }

    /// Validates header names/values and query names.
    pub(crate) fn header_map(&self) -> Result<HeaderMap, KitError> {
        if self.query.keys().any(String::is_empty) {
            return Err(KitError::bad_request("secret", "query"));
        }
        let mut map = HeaderMap::new();
        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| KitError::bad_request("secret", "header"))?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| KitError::bad_request("secret", "header"))?;
            value.set_sensitive(true);
            map.insert(name, value);
        }
        Ok(map)
    }

    fn values(&self) -> Vec<String> {
        self.headers
            .values()
            .chain(self.query.values())
            .cloned()
            .collect()
    }
}

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

/// A tool as discovered, already within the limits of docs/13 §3.
pub(crate) struct Tool {
    pub(crate) name: String,
    pub(crate) description: String,
    /// Serialized JSON schema, at most 32 KiB.
    pub(crate) schema: String,
    pub(crate) read_only: bool,
}

impl Tool {
    fn parse(item: &Value) -> Option<Self> {
        let name = item["name"].as_str().filter(|name| !name.is_empty())?;
        let schema = item
            .get("inputSchema")
            .filter(|schema| schema.is_object())?;
        let schema = schema.to_string();
        if schema.len() > MAX_SCHEMA_BYTES {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            description: item["description"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(MAX_DESCRIPTION_CHARS)
                .collect(),
            schema,
            read_only: item["annotations"]["readOnlyHint"].as_bool() == Some(true),
        })
    }
}

/// Why a request produced no result.
pub(crate) enum RpcError {
    /// The request never reached the server, or the server answered with an error.
    Failed(KitError),
    /// The request was sent but the answer never arrived (timeout, broken connection,
    /// cancel, oversized body): it may have been executed (docs/13 §3).
    Unknown(KitError),
    /// 404 to a request carrying a session id: the session expired.
    Expired,
}

impl RpcError {
    pub(crate) fn into_error(self) -> KitError {
        match self {
            Self::Failed(error) | Self::Unknown(error) => error,
            Self::Expired => KitError::upstream(404, "", &[]),
        }
    }
}

/// One server connection: credentials applied, session id remembered.
pub(crate) struct Connection {
    http: reqwest::Client,
    /// The stored url plus the secret query parameters; never logged.
    url: Url,
    headers: HeaderMap,
    /// Plaintext credentials, replaced in upstream error excerpts.
    known: Vec<String>,
    session: Option<HeaderValue>,
    timeout: Duration,
    next_id: u64,
}

impl Connection {
    /// Validates the url against the policy and performs the handshake.
    pub(crate) async fn open(
        url: &str,
        secret: &Secret,
        allow_private: bool,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Self, KitError> {
        let policy = policy(url, allow_private);
        let mut full = net::check_url(url, &policy)?;
        if !secret.query.is_empty() {
            full.query_pairs_mut().extend_pairs(&secret.query);
        }
        let mut connection = Self {
            http: net::client(policy, CONNECT_TIMEOUT)?,
            url: full,
            headers: secret.header_map()?,
            known: secret.values(),
            session: None,
            timeout,
            next_id: 0,
        };
        connection
            .handshake(cancel)
            .await
            .map_err(RpcError::into_error)?;
        Ok(connection)
    }

    /// All pages of `tools/list`, within the limits of docs/13 §3.
    pub(crate) async fn list_tools(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<Vec<Tool>, KitError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut dropped = 0_usize;
        for _ in 0..MAX_PAGES {
            let params = cursor.map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let result = self
                .call("tools/list", params, cancel)
                .await
                .map_err(RpcError::into_error)?;
            for item in result["tools"].as_array().into_iter().flatten() {
                match Tool::parse(item) {
                    Some(tool) if tools.len() < MAX_TOOLS => tools.push(tool),
                    Some(_) | None => dropped += 1,
                }
            }
            cursor = result["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() || tools.len() >= MAX_TOOLS {
                break;
            }
        }
        if dropped > 0 {
            tracing::warn!(dropped, "mcp.client.tools_dropped");
        }
        Ok(tools)
    }

    /// A JSON-RPC request; a 404 means the session expired and triggers one re-handshake.
    pub(crate) async fn call(
        &mut self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, RpcError> {
        match self.request(method, &params, cancel).await {
            Err(RpcError::Expired) => {
                tracing::debug!("mcp.client.session_expired");
                self.session = None;
                self.handshake(cancel).await?;
                self.request(method, &params, cancel).await
            }
            other => other,
        }
    }

    async fn handshake(&mut self, cancel: &CancellationToken) -> Result<(), RpcError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "aether", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self.request("initialize", &params, cancel).await?;
        let Some(version) = result["protocolVersion"].as_str() else {
            return Err(RpcError::Failed(
                KitError::new(
                    KitErrorCode::Upstream,
                    "initialize result lacks protocolVersion",
                )
                .with_reason("protocol_version"),
            ));
        };
        if !ACCEPTED_VERSIONS.contains(&version) {
            return Err(RpcError::Failed(KitError::unsupported("protocol_version")));
        }
        let body = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let timeout = self.timeout;
        bounded(timeout, cancel, async {
            let response = self.post(&body).await?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(RpcError::Failed(self.upstream(response).await))
            }
        })
        .await
    }

    async fn request(
        &mut self,
        method: &str,
        params: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value, RpcError> {
        self.next_id += 1;
        let id = self.next_id;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let timeout = self.timeout;
        bounded(timeout, cancel, async {
            let had_session = self.session.is_some();
            let response = self.post(&body).await?;
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
        })
        .await
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
                RpcError::Failed(transport(error))
            } else {
                RpcError::Unknown(transport(error))
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
            .map_err(|error| RpcError::Unknown(transport(error.without_url())))?
        {
            total += chunk.len();
            if total > MAX_RESPONSE_BYTES {
                return Err(RpcError::Unknown(too_large()));
            }
            if sse {
                if let Some(message) = find_in_events(parser.feed(&chunk), id) {
                    return self.rpc_result(message);
                }
            } else {
                body.extend_from_slice(&chunk);
            }
        }
        let message = if sse {
            find_in_events(parser.finish(), id).ok_or_else(|| {
                RpcError::Unknown(KitError::new(
                    KitErrorCode::Network,
                    "event stream ended without a response",
                ))
            })?
        } else {
            serde_json::from_slice(&body)
                .ok()
                .and_then(|message| find(message, id))
                .ok_or_else(|| {
                    RpcError::Failed(
                        KitError::new(KitErrorCode::Upstream, "invalid JSON-RPC response")
                            .with_reason("invalid_response"),
                    )
                })?
        };
        self.rpc_result(message)
    }

    fn rpc_result(&self, mut message: Value) -> Result<Value, RpcError> {
        if let Some(error) = message.get("error") {
            let text = error["message"].as_str().unwrap_or_default();
            let known: Vec<&str> = self.known.iter().map(String::as_str).collect();
            return Err(RpcError::Failed(
                KitError::upstream(200, text, &known).with_reason("rpc_error"),
            ));
        }
        Ok(message["result"].take())
    }

    async fn upstream(&self, mut response: Response) -> KitError {
        let status = response.status().as_u16();
        let mut body = Vec::new();
        // The excerpt is best effort: a broken error body still yields the status.
        while let Ok(Some(chunk)) = response.chunk().await {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_RESPONSE_BYTES {
                break;
            }
        }
        let known: Vec<&str> = self.known.iter().map(String::as_str).collect();
        KitError::upstream(status, &String::from_utf8_lossy(&body), &known)
    }
}

/// Applies the per-request deadline and the cancel token. Both drop the request
/// mid-flight, so its outcome is unknown.
async fn bounded<T>(
    timeout: Duration,
    cancel: &CancellationToken,
    work: impl Future<Output = Result<T, RpcError>>,
) -> Result<T, RpcError> {
    tokio::select! {
        () = cancel.cancelled() => Err(RpcError::Unknown(KitError::cancelled())),
        result = tokio::time::timeout(timeout, work) => result.unwrap_or_else(|_| {
            Err(RpcError::Unknown(KitError::new(KitErrorCode::Timeout, "request timed out")))
        }),
    }
}

fn transport(error: reqwest::Error) -> KitError {
    let code = if error.is_timeout() {
        KitErrorCode::Timeout
    } else {
        KitErrorCode::Network
    };
    KitError::new(code, "MCP server unreachable").with_cause(error)
}

fn too_large() -> KitError {
    KitError::new(KitErrorCode::BudgetExceeded, "response too large")
        .with_limit(MAX_RESPONSE_BYTES as u64)
}

fn find_in_events(events: Vec<SseEvent>, id: u64) -> Option<Value> {
    events.into_iter().find_map(|event| {
        serde_json::from_str(&event.data)
            .ok()
            .and_then(|message| find(message, id))
    })
}

/// The response with `id` in a single message or a batch.
fn find(message: Value, id: u64) -> Option<Value> {
    let matches = |item: &Value| item["id"].as_u64() == Some(id);
    if let Value::Array(items) = message {
        items.into_iter().find(matches)
    } else {
        Some(message).filter(matches)
    }
}
