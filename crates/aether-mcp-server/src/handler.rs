//! MCP tool dispatch and protocol mapping (docs/28 §§4.2–5).

use std::sync::Arc;

use aether_support::{KitError, KitErrorCode};
use hyper::http::request::Parts;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode, ErrorData,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde_json::Value;

use crate::store::Grant;
use crate::{CallContext, Inner};

/// JSON-RPC code for a missing scope (docs/28 §4.2).
const FORBIDDEN: ErrorCode = ErrorCode(-32001);

/// The rmcp handler; one clone per request (stateless mode).
#[derive(Clone)]
pub(crate) struct Mcp(pub(crate) Arc<Inner>);

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

/// docs/28 §5 error mapping; `data` is the serialized `KitError` (safe by construction).
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
