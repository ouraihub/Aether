//! Builder validation, a run without a conversation store, host tools and approvals
//! (docs/15 §2, §4).
#![cfg(feature = "chat")]
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap is a failed test"
)]

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use aether_chat::{Approver, EndStatus, RunEvent, Runner, ToolOutcome, ToolSource, ToolSpec};
use aether_observability::{LogConfig, LogHandle};
use aether_support::KitErrorCode;
use aether_support::net::AddressPolicy;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::tools::{Approvals, HostTools};
use crate::{
    Approval, Builder, CancellationToken, ChatConfig, HostTool, KitHost, RunRequest, ToolCall,
    ToolError, ToolOutput,
};

struct Host;
impl KitHost for Host {}
// One global subscriber per process: shared by every test here.
static LOGS: LazyLock<(tempfile::TempDir, LogHandle)> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    let handle = aether_observability::init(dir.path(), LogConfig::default(), None).unwrap();
    (dir, handle)
});

fn logs() -> LogHandle {
    LOGS.1.clone()
}

#[test]
fn build_without_providers_is_missing_instance() {
    let error = Builder::new(Arc::new(Host), logs())
        .chat(ChatConfig::default(), None)
        .build::<tauri::Wry>()
        .err()
        .unwrap();
    assert_eq!(error.code(), KitErrorCode::BadRequest);
    assert_eq!(error.details().field(), Some("providers"));
    assert_eq!(error.details().reason(), Some("missing_instance"));
}

#[test]
fn build_without_chat_is_missing_instance() {
    let dir = tempfile::tempdir().unwrap();
    let error = Builder::new(Arc::new(Host), logs())
        .providers(aether_providers::Store::open(dir.path()).unwrap())
        .build::<tauri::Wry>()
        .err()
        .unwrap();
    assert_eq!(error.details().field(), Some("chat"));
    assert_eq!(error.details().reason(), Some("missing_instance"));
}

const HELLO: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

/// openai-chat streaming responses, one per connection; returns the requests received.
async fn upstream(listener: TcpListener, bodies: Vec<&'static str>) -> Vec<String> {
    let mut requests = Vec::new();
    for body in bodies {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0_u8; 4096];
        // The body is small JSON ending in `}`; stop once headers and body have arrived.
        while !(request.ends_with(b"}") && request.windows(4).any(|w| w == b"\r\n\r\n")) {
            let n = socket.read(&mut buf).await.unwrap();
            request.extend_from_slice(&buf[..n]);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        requests.push(String::from_utf8(request).unwrap());
    }
    requests
}

/// A provider store with one openai-chat model at `base_url`, and a run request for it.
fn local_model(
    dir: &std::path::Path,
    base_url: &str,
    input: &str,
) -> (aether_providers::Store, RunRequest) {
    let providers = aether_providers::Store::open(dir).unwrap();
    let saved = providers
        .save(
            serde_json::from_value(json!({ "name": "Local", "endpoints": [{
                "slug": "main", "baseUrl": base_url, "protocol": "openai-chat", "apiKey": "sk-test",
                "models": [{ "id": "m1", "modelType": "chat", "typeSource": "user",
                    "contextWindow": null, "maxOutput": null, "canVision": false, "supportsTools": null }]
            }] }))
            .unwrap(),
        )
        .unwrap();
    let endpoint_id = saved[0].endpoints[0].id.clone();
    let request = serde_json::from_value(json!({
        "purpose": "chat", "model": { "endpointId": endpoint_id, "modelId": "m1" },
        "skills": [], "input": input, "regenerate": false, "language": "en"
    }))
    .unwrap();
    (providers, request)
}

#[tokio::test]
async fn runs_without_a_conversation_store() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));

    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &base_url, "hi");

    super::chat::check_request(&request, false).unwrap();
    let regenerate = RunRequest {
        regenerate: true,
        conversation_id: Some("c1".to_owned()),
        ..request.clone()
    };
    let error = super::chat::check_request(&regenerate, false).unwrap_err();
    assert_eq!(error.details().reason(), Some("no_conversation"));

    let input = super::chat::assemble(
        &Host,
        &providers,
        None,
        Vec::new(),
        ChatConfig::default().budget,
        &request,
        "r1".to_owned(),
    )
    .unwrap();
    assert!(input.history.is_empty());
    assert!(input.tools.is_empty());
    let runner = Runner::new(AddressPolicy::default()).unwrap();
    let mut text = String::new();
    let summary = runner
        .run(
            input,
            &HostTools::new(Arc::new(Host)),
            &Approvals::new(Duration::from_secs(1)),
            &aether_chat::CancellationToken::new(),
            |event| {
                if let aether_chat::RunEvent::TextDelta { text: delta } = event {
                    text.push_str(&delta);
                }
            },
        )
        .await;
    assert_eq!(summary.status, EndStatus::Completed);
    assert_eq!(text, "Hello");
    let received = server.await.unwrap();
    assert!(received[0].starts_with("POST /v1/chat/completions"));
    assert!(received[0].contains("\"content\":\"hi\""));
}

/// `read_note` (auto) and `write_note` (ask); `authorize` denies `write_note` when `deny`.
struct Notes {
    deny: bool,
    calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl KitHost for Notes {
    fn tools(&self, _req: &RunRequest) -> Vec<HostTool> {
        [("read_note", Approval::Auto), ("write_note", Approval::Ask)]
            .map(|(name, approval)| HostTool {
                spec: ToolSpec {
                    name: name.to_owned(),
                    description: format!("{name} tool"),
                    parameters: json!({ "type": "object", "properties": {} }),
                    approval,
                    source: ToolSource::Host,
                },
            })
            .into()
    }
    async fn call_tool(
        &self,
        call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.lock().unwrap().push(call.name.clone());
        Ok(ToolOutput {
            text: format!("{} done", call.name),
            payload: None,
        })
    }
    fn authorize(&self, call: &ToolCall, default: Approval) -> Approval {
        if self.deny && call.name == "write_note" {
            Approval::Deny
        } else {
            default
        }
    }
}

/// First round calls both tools, second round answers with text.
const TWO_TOOLS: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read_note\",\"arguments\":\"{}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"c2\",\"type\":\"function\",\"function\":{\"name\":\"write_note\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: [DONE]\n\n",
);

/// Runs the two-tool script; `answer` replies to `approval_required` through `Approvals`.
async fn tool_run(
    host: Notes,
    answer: Option<bool>,
) -> (Arc<Notes>, Vec<(String, ToolOutcome)>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![TWO_TOOLS, HELLO]));
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &base_url, "note");
    let host = Arc::new(host);
    let input = super::chat::assemble(
        host.as_ref(),
        &providers,
        None,
        Vec::new(),
        ChatConfig::default().budget,
        &request,
        "r1".to_owned(),
    )
    .unwrap();
    assert_eq!(input.tools.len(), 2);
    let approvals = Arc::new(Approvals::new(Duration::from_secs(5)));
    let mut outcomes = Vec::new();
    let summary = Runner::new(AddressPolicy::default())
        .unwrap()
        .run(
            input,
            &HostTools::new(host.clone()),
            &Answering(approvals.clone(), answer),
            &CancellationToken::new(),
            |event| {
                if let RunEvent::ToolResult {
                    call_id, outcome, ..
                } = event
                {
                    outcomes.push((call_id, outcome));
                }
            },
        )
        .await;
    assert_eq!(summary.status, EndStatus::Completed);
    let requests = server.await.unwrap();
    (host, outcomes, requests[1].clone())
}

/// Waits through `Approvals` and answers right after the wait is registered, as `chat_approve` would.
struct Answering(Arc<Approvals>, Option<bool>);

impl Approver for Answering {
    fn wait(
        &self,
        run_id: &str,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> aether_chat::BoxFuture<'_, bool> {
        let waiting = self.0.wait(run_id, call, cancel);
        // `None`: the test expects no `Ask` to reach the approver at all.
        let allow = self.1.expect("approval was asked");
        self.0.answer(run_id, &call.id, allow);
        waiting
    }
}

#[tokio::test]
async fn host_tools_run_auto_and_ask_after_approval() {
    let host = Notes {
        deny: false,
        calls: Mutex::default(),
    };
    let (host, outcomes, second) = tool_run(host, Some(true)).await;
    assert_eq!(*host.calls.lock().unwrap(), ["read_note", "write_note"]);
    assert_eq!(
        outcomes,
        [
            ("c1".into(), ToolOutcome::Ok),
            ("c2".into(), ToolOutcome::Ok)
        ]
    );
    assert!(second.contains("write_note done"));
}

#[tokio::test]
async fn user_deny_and_host_deny_never_execute() {
    let host = Notes {
        deny: false,
        calls: Mutex::default(),
    };
    let (host, outcomes, second) = tool_run(host, Some(false)).await;
    assert_eq!(*host.calls.lock().unwrap(), ["read_note"]);
    assert_eq!(outcomes[1], ("c2".into(), ToolOutcome::Denied));
    assert!(second.contains("该工具调用已被拒绝，未执行。"));

    // `authorize` → Deny: no approval is asked (no answer given) and nothing runs.
    let host = Notes {
        deny: true,
        calls: Mutex::default(),
    };
    let (host, outcomes, _) = tool_run(host, None).await;
    assert_eq!(*host.calls.lock().unwrap(), ["read_note"]);
    assert_eq!(outcomes[1], ("c2".into(), ToolOutcome::Denied));
}

#[tokio::test]
async fn approval_times_out_as_denied_and_stops_on_cancel() {
    let approvals = Approvals::new(Duration::from_millis(50));
    let call = ToolCall {
        id: "c1".into(),
        name: "write_note".into(),
        args: json!({}),
    };
    let cancel = CancellationToken::new();
    assert!(!approvals.wait("r1", &call, &cancel).await);
    // A late answer to a settled wait is ignored.
    approvals.answer("r1", "c1", true);

    let waiting = approvals.wait("r1", &call, &cancel);
    cancel.cancel();
    assert!(!waiting.await);
}
