//! Tool executor and approver composed for `chat_run`, and `chat_approve` (docs/15 §4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use aether_chat::{Approver, BoxFuture, ToolExecution, ToolExecutor};
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::oneshot;

use crate::chat::Chat;
use crate::host::{Ack, Approval, CancellationToken, KitHost, ToolCall};

/// The run's tools behind the runner: `KitHost::authorize` has the final say for every
/// tool; host tools run through `call_tool`, MCP tools through the run's `McpSession`.
pub(crate) struct HostTools {
    pub(crate) host: Arc<dyn KitHost>,
    /// Calls in one run are sequential, so the lock is never contended.
    #[cfg(feature = "mcp-client")]
    pub(crate) mcp: Option<tokio::sync::Mutex<aether_mcp_client::McpSession>>,
    /// Set when this run offers `generate_image` (docs/20 §3).
    pub(crate) painting: Option<Painting>,
}

/// What `generate_image` needs during one run.
pub(crate) struct Painting {
    pub(crate) connection: aether_support::Connection,
    pub(crate) policy: aether_support::net::AddressPolicy,
    pub(crate) store: Option<(Arc<dyn aether_chat::ConversationStore>, String)>,
    /// ponytail: per-run cap stops a looping model; raise only if users ask for batches.
    pub(crate) calls: std::sync::atomic::AtomicU8,
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
    // Called inline, not in `spawn_blocking`: the runner's `authorize` is synchronous.
    fn authorize(&self, call: &ToolCall, default: Approval) -> Approval {
        self.host.authorize(call, default)
    }

    async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolExecution {
        if call.name == aether_chat::GENERATE_IMAGE
            && let Some(painting) = &self.painting
        {
            return paint(painting, call, cancel).await;
        }
        // Dispatch by catalogue: the names are unique across sources (Chat `validate`).
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
            },
        }
    }
}

/// One `generate_image` call: paint, store as the turn's generated image, show it in the UI.
async fn paint(painting: &Painting, call: &ToolCall, cancel: &CancellationToken) -> ToolExecution {
    if painting
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        >= 2
    {
        return ToolExecution::Error {
            code: "limit".to_owned(),
            message: "At most 2 images per reply.".to_owned(),
        };
    }
    let generated = tokio::select! {
        () = cancel.cancelled() => return ToolExecution::Unknown { reason: "cancelled".to_owned() },
        result = aether_chat::generate_image(&painting.connection, &painting.policy, &call.args) => result,
    };
    let image = match generated {
        Ok(image) => image,
        Err(error) => {
            return ToolExecution::Error {
                code: format!("{:?}", error.code()).to_lowercase(),
                message: error.message().to_owned(),
            };
        }
    };
    if let Some((store, turn_id)) = &painting.store {
        let (store, turn_id, part) = (Arc::clone(store), turn_id.clone(), image.clone());
        let saved =
            tauri::async_runtime::spawn_blocking(move || store.add_generated(&turn_id, &[part]))
                .await;
        if !matches!(saved, Ok(Ok(()))) {
            tracing::warn!("chat.images.generated_store_failed");
        }
    }
    let url = format!("data:{};base64,{}", image.mime, image.data);
    ToolExecution::Ok {
        text: "Generated 1 image; it is already shown to the user.".to_owned(),
        payload: Some(serde_json::json!({ "images": [{ "mime": image.mime, "url": url }] })),
    }
}

/// Plain text back to the runner, which wraps MCP output as external content
/// (`ToolSource::Mcp`, docs/12 §5); wrapping here as well would nest two boundaries.
#[cfg(feature = "mcp-client")]
fn mcp_execution(result: aether_mcp_client::McpCallResult) -> ToolExecution {
    use aether_mcp_client::McpCallResult;
    match result {
        McpCallResult::Ok { text } => ToolExecution::Ok {
            text,
            payload: None,
        },
        McpCallResult::Error { code, message } => ToolExecution::Error { code, message },
        McpCallResult::Unknown { reason } => ToolExecution::Unknown { reason },
    }
}

/// docs/15 §4 step 5: MCP tools of enabled, trusted servers as `ToolSource::Mcp` specs, and
/// the session that runs them. This process's own MCP Server is skipped while it runs
/// (docs/13 §9.2).
#[cfg(feature = "mcp-client")]
pub(crate) async fn mcp_tools<R: tauri::Runtime>(
    app: &AppHandle<R>,
    cancel: &CancellationToken,
) -> (Vec<crate::ToolSpec>, aether_mcp_client::McpSession) {
    #[cfg(feature = "mcp-server")]
    let own_port = {
        let status = app.state::<aether_mcp_server::McpServer>().status();
        status.running.then_some(status.port)
    };
    #[cfg(not(feature = "mcp-server"))]
    let own_port = None;
    let session = app
        .state::<crate::mcp_client::McpClient>()
        .store
        .session(cancel, own_port)
        .await;
    let specs = session
        .tools()
        .into_iter()
        .map(|tool| crate::ToolSpec {
            name: tool.prefixed,
            description: tool.description,
            parameters: tool.schema,
            approval: tool.approval,
            source: crate::ToolSource::Mcp {
                server: tool.server,
            },
        })
        .collect();
    (specs, session)
}

/// `Ask` calls waiting for `chat_approve`, keyed by (run id, call id).
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

    /// Answers a waiting call; unknown or already settled calls are ignored.
    pub(crate) fn answer(&self, run_id: &str, call_id: &str, allow: bool) {
        if let Some(sender) = self
            .pending()
            .remove(&(run_id.to_owned(), call_id.to_owned()))
        {
            // The waiter may have just timed out or been cancelled; then nobody listens.
            let _ = sender.send(allow);
        }
    }
}

impl Approver for Approvals {
    /// Allowed only by `chat_approve(allow: true)`; cancel and timeout count as denied.
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
            let allow = tokio::select! {
                () = cancel.cancelled() => false,
                answer = tokio::time::timeout(self.timeout, receiver) => matches!(answer, Ok(Ok(true))),
            };
            self.pending().remove(&key);
            allow
        })
    }
}

/// Allows or denies a call awaiting approval; unknown ids are fine (it may have timed out).
#[tauri::command]
#[tracing::instrument(skip_all, fields(command = "chat_approve"), err(level = "warn"))]
pub(crate) async fn chat_approve<R: Runtime>(
    app: AppHandle<R>,
    run_id: String,
    call_id: String,
    allow: bool,
) -> Result<Ack, crate::KitError> {
    app.state::<Chat>()
        .approvals
        .answer(&run_id, &call_id, allow);
    Ok(Ack { ok: true })
}
