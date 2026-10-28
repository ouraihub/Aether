//! Fixed-catalogue tool dispatch and service-owned approval waits.

pub(crate) mod painting;
use crate::{Approval, CancellationToken, KitHost, ToolCall};
use aether_chat::{Approver, AuthorizeContext, BoxFuture, ToolExecution, ToolExecutor};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::oneshot;

pub(crate) struct HostTools {
    pub(crate) host: Arc<dyn KitHost>,
    #[cfg(feature = "mcp-client")]
    pub(crate) mcp: Option<tokio::sync::Mutex<aether_mcp_client::McpSession>>,
    pub(crate) painting: Option<painting::Painting>,
}
impl HostTools {
    pub(crate) fn new(host: Arc<dyn KitHost>) -> Self {
        Self {
            host,
            #[cfg(feature = "mcp-client")]
            mcp: None,
            painting: None,
        }
    }
}
#[async_trait::async_trait]
impl ToolExecutor for HostTools {
    fn authorize(&self, call: &ToolCall, context: &AuthorizeContext<'_>) -> Approval {
        self.host.authorize(call, context)
    }
    async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolExecution {
        if call.name == "generate_image"
            && let Some(painting) = &self.painting
        {
            return painting.execute(call, cancel).await;
        }
        #[cfg(feature = "mcp-client")]
        if let Some(mcp) = &self.mcp {
            let mut session = mcp.lock().await;
            if session
                .tools()
                .iter()
                .any(|tool| tool.prefixed == call.name)
            {
                return mcp_execution(session.call(&call.name, call.args.clone(), cancel).await);
            }
        }
        match self.host.call_tool(call, cancel).await {
            Ok(output) => ToolExecution::Ok {
                text: output.text,
                payload: output.payload,
            },
            Err(error) if error.unknown => ToolExecution::Unknown {
                reason: error.message,
            },
            Err(error) => ToolExecution::Error {
                code: error.code,
                message: error.message,
                payload: None,
            },
        }
    }
}

/// The only MCP-to-model/UI projection. Chat adds the external-content boundary once.
#[cfg(feature = "mcp-client")]
fn mcp_execution(result: aether_mcp_client::McpCallResult) -> ToolExecution {
    use aether_mcp_client::McpCallResult;
    match result {
        McpCallResult::Completed {
            content,
            structured_content,
            is_error,
        } => {
            let text = content
                .iter()
                .map(|block| {
                    if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                        block
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| block.to_string())
                    } else {
                        block.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let payload = Some(
                serde_json::json!({ "content": content, "structuredContent": structured_content }),
            );
            if is_error {
                ToolExecution::Error {
                    code: "mcp_error".into(),
                    message: text,
                    payload,
                }
            } else {
                ToolExecution::Ok { text, payload }
            }
        }
        McpCallResult::Failed { code, message } => ToolExecution::Error {
            code,
            message,
            payload: None,
        },
        McpCallResult::Unknown { reason } => ToolExecution::Unknown { reason },
    }
}

pub(crate) struct Approvals {
    pending: Mutex<HashMap<(String, String), oneshot::Sender<bool>>>,
    timeout: Duration,
}
impl Approvals {
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            pending: Mutex::default(),
            timeout,
        }
    }
    fn pending(&self) -> MutexGuard<'_, HashMap<(String, String), oneshot::Sender<bool>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub(crate) fn answer(&self, run_id: &str, call_id: &str, allow: bool) -> bool {
        self.pending()
            .remove(&(run_id.to_owned(), call_id.to_owned()))
            .is_some_and(|sender| sender.send(allow).is_ok())
    }
    pub(crate) fn remove_run(&self, run_id: &str) {
        self.pending().retain(|(id, _), _| id != run_id);
    }
}
impl Approver for Approvals {
    fn wait(
        &self,
        run_id: &str,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> BoxFuture<'_, bool> {
        let key = (run_id.to_owned(), call.id.clone());
        let (sender, receiver) = oneshot::channel();
        self.pending().insert(key.clone(), sender);
        let cancel = cancel.clone();
        Box::pin(async move {
            let result = tokio::select! { biased; () = cancel.cancelled() => false, answer = tokio::time::timeout(self.timeout, receiver) => matches!(answer, Ok(Ok(true))) };
            self.pending().remove(&key);
            result
        })
    }
}

#[cfg(all(test, feature = "mcp-client"))]
#[path = "tests/mcp.rs"]
mod tests;
