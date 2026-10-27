//! Transport-independent JSON-RPC session (docs/13 §3): handshake, paged discovery, calls,
//! deadlines and cancellation. `http` and `stdio` only move messages.

use std::time::Duration;

use aether_support::{KitError, KitErrorCode};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::http::HttpLink;
use crate::stdio::StdioLink;

const PROTOCOL_VERSION: &str = "2025-03-26";
const ACCEPTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26"];
/// Per request, from sending until the answer is read.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// A JSON body, SSE stream or stdio line.
pub(crate) const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 10;
const MAX_TOOLS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_SCHEMA_BYTES: usize = 32 * 1024;

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
    /// cancel, oversized message): it may have been executed (docs/13 §3).
    Unknown(KitError),
    /// The server forgot the session (http 404 with a session id).
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

/// Moves JSON-RPC messages to one server. Deadlines and cancellation are applied by
/// [`Connection`] around these futures, so they must tolerate being dropped mid-flight.
pub(crate) trait Transport {
    /// Sends a request and returns the response message whose `id` matches.
    async fn request(&mut self, message: &Value, id: u64) -> Result<Value, RpcError>;
    /// Sends a notification; no response is expected.
    async fn notify(&mut self, message: &Value) -> Result<(), RpcError>;
    /// Forgets server-side session state before a new handshake.
    fn reset(&mut self);
    /// A request was dropped without an answer; the link decides whether it stays usable.
    fn abandon(&mut self);
    /// Whether later requests may still succeed.
    fn is_alive(&self) -> bool;
    /// Plaintext credentials, replaced in error excerpts.
    fn known(&self) -> &[String];
}

/// The transports of docs/13 §2.
pub(crate) enum Link {
    Http(Box<HttpLink>),
    Stdio(Box<StdioLink>),
}

impl Transport for Link {
    async fn request(&mut self, message: &Value, id: u64) -> Result<Value, RpcError> {
        match self {
            Self::Http(link) => link.request(message, id).await,
            Self::Stdio(link) => link.request(message, id).await,
        }
    }

    async fn notify(&mut self, message: &Value) -> Result<(), RpcError> {
        match self {
            Self::Http(link) => link.notify(message).await,
            Self::Stdio(link) => link.notify(message).await,
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Http(link) => link.reset(),
            Self::Stdio(link) => link.reset(),
        }
    }

    fn abandon(&mut self) {
        match self {
            Self::Http(link) => link.abandon(),
            Self::Stdio(link) => link.abandon(),
        }
    }

    fn is_alive(&self) -> bool {
        match self {
            Self::Http(link) => link.is_alive(),
            Self::Stdio(link) => link.is_alive(),
        }
    }

    fn known(&self) -> &[String] {
        match self {
            Self::Http(link) => link.known(),
            Self::Stdio(link) => link.known(),
        }
    }
}

/// One server connection after a successful handshake.
pub(crate) struct Connection {
    link: Link,
    timeout: Duration,
    next_id: u64,
}

impl Connection {
    /// Performs the handshake within `startup`, then uses `timeout` per request.
    pub(crate) async fn open(
        link: Link,
        timeout: Duration,
        startup: Duration,
        cancel: &CancellationToken,
    ) -> Result<Self, KitError> {
        let mut connection = Self {
            link,
            timeout,
            next_id: 0,
        };
        // One deadline over the whole handshake: a server that never answers `initialize`
        // is given up on after `startup`, not after each step.
        bounded(startup, cancel, connection.handshake())
            .await
            .map_err(RpcError::into_error)?;
        Ok(connection)
    }

    /// Whether the connection can be reused for the next call.
    pub(crate) fn is_alive(&self) -> bool {
        self.link.is_alive()
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

    /// A JSON-RPC request; an expired session triggers one re-handshake.
    pub(crate) async fn call(
        &mut self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, RpcError> {
        let timeout = self.timeout;
        match bounded(timeout, cancel, self.request(method, &params)).await {
            Err(RpcError::Expired) => {
                tracing::debug!("mcp.client.session_expired");
                self.link.reset();
                bounded(timeout, cancel, self.handshake()).await?;
                bounded(timeout, cancel, self.request(method, &params)).await
            }
            other => other,
        }
        .inspect_err(|error| {
            if matches!(error, RpcError::Unknown(_)) {
                self.link.abandon();
            }
        })
    }

    async fn handshake(&mut self) -> Result<(), RpcError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "aether", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self.request("initialize", &params).await?;
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
        self.link
            .notify(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
    }

    async fn request(&mut self, method: &str, params: &Value) -> Result<Value, RpcError> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut reply = self.link.request(&message, id).await?;
        if let Some(error) = reply.get("error") {
            let text = error["message"].as_str().unwrap_or_default();
            let known: Vec<&str> = self.link.known().iter().map(String::as_str).collect();
            return Err(RpcError::Failed(
                KitError::upstream(200, text, &known).with_reason("rpc_error"),
            ));
        }
        Ok(reply["result"].take())
    }
}

/// Applies a deadline and the cancel token. Both drop the request mid-flight, so its
/// outcome is unknown.
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

pub(crate) fn too_large() -> KitError {
    KitError::new(KitErrorCode::BudgetExceeded, "response too large")
        .with_limit(MAX_MESSAGE_BYTES as u64)
}

pub(crate) fn invalid_response() -> KitError {
    KitError::new(KitErrorCode::Upstream, "invalid JSON-RPC response")
        .with_reason("invalid_response")
}

/// The response with `id` in a single message or a batch.
pub(crate) fn find(message: Value, id: u64) -> Option<Value> {
    let matches = |item: &Value| item["id"].as_u64() == Some(id);
    if let Value::Array(items) = message {
        items.into_iter().find(matches)
    } else {
        Some(message).filter(matches)
    }
}
