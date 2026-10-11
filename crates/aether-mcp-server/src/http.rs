//! Loopback HTTP front for rmcp's Streamable HTTP service: peer, `Origin` and bearer
//! checks happen here, scope checks in the MCP handler (docs/13 §8).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aether_support::{KitError, KitErrorCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{AUTHORIZATION, HeaderValue, ORIGIN, WWW_AUTHENTICATE};
use hyper::http::request::Parts;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode, ErrorData,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{RoleServer, ServerHandler};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::store::Grant;
use crate::{CallContext, DEFAULT_MAX_REQUEST_BYTES, Inner};

type Body = BoxBody<Bytes, Infallible>;
type Service = StreamableHttpService<Mcp, NeverSessionManager>;

/// Room for the JSON-RPC envelope around a tool's arguments.
const ENVELOPE_BYTES: usize = 8 * 1024;
/// JSON-RPC code for a missing scope (docs/13 §8).
const FORBIDDEN: ErrorCode = ErrorCode(-32001);

/// Accept loop; returns (dropping the listener) once `cancel` fires.
pub(crate) async fn serve(listener: TcpListener, inner: Arc<Inner>, cancel: CancellationToken) {
    let service = mcp_service(&inner, &cancel);
    loop {
        let (stream, peer) = tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(error) => {
                    // Usually fd exhaustion; back off instead of spinning.
                    tracing::warn!(error = %error, "mcp.server.accept_failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let inner = Arc::clone(&inner);
        let service = service.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let handler = service_fn(move |request| {
                let inner = Arc::clone(&inner);
                let service = service.clone();
                async move { Ok::<_, Infallible>(route(inner, &service, peer, request).await) }
            });
            let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), handler);
            tokio::select! {
                () = cancel.cancelled() => {}
                result = connection => if let Err(error) = result {
                    tracing::debug!(error = %error, "mcp.server.connection_closed");
                },
            }
        });
    }
}

fn mcp_service(inner: &Arc<Inner>, cancel: &CancellationToken) -> Service {
    let largest = inner
        .tools
        .iter()
        .map(|tool| tool.max_request_bytes)
        .max()
        .unwrap_or(DEFAULT_MAX_REQUEST_BYTES);
    // Stateless JSON responses: every request carries its own bearer token, so there is
    // no session to hijack and revocation needs no session teardown.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(cancel.child_token())
        .with_max_request_body_bytes(largest.saturating_add(ENVELOPE_BYTES));
    let handler = Mcp(Arc::clone(inner));
    StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}

async fn route(
    inner: Arc<Inner>,
    service: &Service,
    peer: SocketAddr,
    mut request: Request<Incoming>,
) -> Response<Body> {
    if let Err(reason) = gate(peer, request.headers()) {
        tracing::warn!(reason, "mcp.server.rejected");
        return reply(StatusCode::FORBIDDEN, "forbidden");
    }
    if request.uri().path() != "/mcp" {
        return reply(StatusCode::NOT_FOUND, "not found");
    }
    let Some(token) = bearer(request.headers()) else {
        return unauthorized();
    };
    let grant = match tokio::task::spawn_blocking(move || inner.store.authenticate(&token)).await {
        Ok(Ok(Some(grant))) => grant,
        Ok(Ok(None)) => return unauthorized(),
        // `KitError::internal` already logged the cause.
        Ok(Err(_)) => return reply(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        Err(error) => {
            let _ = KitError::internal(error);
            return reply(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    request.extensions_mut().insert(grant);
    service.handle(request).await
}

/// Peer must be loopback; every `Origin` present must be a loopback origin, which defeats
/// DNS rebinding from a browser page (docs/13 §8).
fn gate(peer: SocketAddr, headers: &HeaderMap) -> Result<(), &'static str> {
    if !peer.ip().is_loopback() {
        return Err("peer");
    }
    let origins_ok = headers
        .get_all(ORIGIN)
        .iter()
        .all(|value| value.to_str().is_ok_and(is_loopback_origin));
    if !origins_ok {
        return Err("origin");
    }
    Ok(())
}

fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        // Includes the opaque `null` origin.
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && match url.host() {
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let (scheme, token) = headers.get(AUTHORIZATION)?.to_str().ok()?.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_owned())
}

fn reply(status: StatusCode, message: &'static str) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from_static(message.as_bytes())).boxed());
    *response.status_mut() = status;
    response
}

fn unauthorized() -> Response<Body> {
    let mut response = reply(StatusCode::UNAUTHORIZED, "unauthorized");
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// The rmcp handler; one clone per request (stateless mode).
#[derive(Clone)]
pub(crate) struct Mcp(Arc<Inner>);

impl ServerHandler for Mcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aether", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let grant = grant(&context)?;
        Ok(ListToolsResult::with_all_items(
            self.0
                .tools
                .iter()
                .filter(|tool| grant.allows(tool.scope))
                .map(describe)
                .collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let grant = grant(&context)?;
        let Some(tool) = self.0.tools.iter().find(|tool| tool.name == request.name) else {
            return Err(to_error(&KitError::not_found("tool")));
        };
        if !grant.allows(tool.scope) {
            tracing::warn!(tool = tool.name, scope = tool.scope, "mcp.server.forbidden");
            return Err(ErrorData::new(FORBIDDEN, "forbidden", None));
        }
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let size = serde_json::to_vec(&arguments).map_or(usize::MAX, |bytes| bytes.len());
        if size > tool.max_request_bytes {
            let limit = u64::try_from(tool.max_request_bytes).unwrap_or(u64::MAX);
            return Err(to_error(
                &KitError::bad_request("arguments", "too_large").with_limit(limit),
            ));
        }
        let call = CallContext {
            token_id: grant.token_id.clone(),
            scopes: grant.scopes.clone(),
            cancel: context.ct.clone(),
        };
        match (tool.handler)(arguments, call).await {
            Ok(value) => Ok(result(value).into()),
            Err(error) => {
                tracing::debug!(tool = tool.name, code = ?error.code(), "mcp.server.call_failed");
                Err(to_error(&error))
            }
        }
    }
}

fn grant(context: &RequestContext<RoleServer>) -> Result<&Grant, ErrorData> {
    context
        .extensions
        .get::<Parts>()
        .and_then(|parts| parts.extensions.get::<Grant>())
        .ok_or_else(|| ErrorData::internal_error("internal error", None))
}

fn describe(tool: &crate::ExposedTool) -> Tool {
    let schema = tool.input_schema.as_object().cloned().unwrap_or_default();
    Tool::new(tool.name, tool.description, Arc::new(schema)).with_annotations(
        ToolAnnotations::new()
            .read_only(tool.read_only)
            .destructive(tool.destructive)
            .idempotent(tool.idempotent),
    )
}

/// Objects become `structuredContent`; strings are sent as plain text.
fn result(value: Value) -> CallToolResult {
    match value {
        Value::Object(_) => CallToolResult::structured(value),
        Value::String(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
            CallToolResult::success(vec![ContentBlock::text(value.to_string())])
        }
    }
}

/// docs/13 §8 error mapping; `data` is the serialized `KitError` (safe by construction).
fn to_error(error: &KitError) -> ErrorData {
    let code = match error.code() {
        KitErrorCode::BadRequest => ErrorCode::INVALID_PARAMS,
        KitErrorCode::NotFound => ErrorCode(-32004),
        KitErrorCode::Conflict => ErrorCode(-32009),
        KitErrorCode::Forbidden
        | KitErrorCode::Unsupported
        | KitErrorCode::Cancelled
        | KitErrorCode::Unauthorized
        | KitErrorCode::RateLimited
        | KitErrorCode::Upstream
        | KitErrorCode::Timeout
        | KitErrorCode::Network
        | KitErrorCode::NeedsConfig
        | KitErrorCode::BudgetExceeded
        | KitErrorCode::IncompatibleDb
        | KitErrorCode::Internal => ErrorCode::INTERNAL_ERROR,
    };
    ErrorData::new(
        code,
        error.message().to_owned(),
        serde_json::to_value(error).ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_rejects_non_loopback_peers_and_origins() {
        let local: SocketAddr = ([127, 0, 0, 1], 5000).into();
        let lan: SocketAddr = ([192, 168, 1, 2], 5000).into();
        let none = HeaderMap::new();
        assert_eq!(gate(lan, &none), Err("peer"));
        assert_eq!(gate(local, &none), Ok(()));

        let mut two = HeaderMap::new();
        two.append(ORIGIN, HeaderValue::from_static("http://localhost"));
        two.append(ORIGIN, HeaderValue::from_static("http://evil.example"));
        assert_eq!(
            gate(local, &two),
            Err("origin"),
            "every Origin must be loopback"
        );
    }
}
