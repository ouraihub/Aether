//! Native runtime behavior through RunService and transactional preparation (docs/25–26).
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "failed assertions fail tests"
)]

use crate::conversations::{ConversationStore, SqliteStore, TurnHandle, TurnRecord, TurnStatus};
use crate::{
    Approval, AuthorizeContext, CancellationToken, EventSink, HostTool, KitHost, ModelRef,
    RunAccepted, RunConfig, RunContext, RunDependencies, RunOwner, RunRequest, RunService,
    RunStatus, ToolCall, ToolError, ToolOutput,
};
use aether_chat::{
    Approver, EndStatus, Message, PermissionMode, RunEvent, ToolEffect, ToolExecution,
    ToolExecutor, ToolOutcome, ToolSource, ToolSpec,
};
use aether_images::ImagePart;
use aether_support::{KitError, KitErrorCode, net::AddressPolicy};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};

mod actions;
mod lifecycle;
#[cfg(feature = "mcp-client")]
mod mcp_session;

struct Host;
impl KitHost for Host {}

const HELLO: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);
const TWO_TOOLS: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read_note\",\"arguments\":\"{}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"c2\",\"type\":\"function\",\"function\":{\"name\":\"write_note\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: [DONE]\n\n",
);

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let (headers, length) = loop {
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..at]);
            let length = header
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .unwrap()
                .1
                .trim()
                .parse::<usize>()
                .unwrap();
            break (at + 4, length);
        }
        let n = socket.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0, "connection closed before request headers");
        bytes.extend_from_slice(&chunk[..n]);
    };
    while bytes.len() < headers + length {
        let n = socket.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0, "connection closed before request body");
        bytes.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8(bytes).unwrap()
}

async fn respond(socket: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await.unwrap();
}

async fn upstream(listener: TcpListener, bodies: Vec<&'static str>) -> Vec<String> {
    let mut requests = Vec::new();
    for body in bodies {
        let (mut socket, _) = listener.accept().await.unwrap();
        requests.push(read_request(&mut socket).await);
        respond(&mut socket, body).await;
    }
    requests
}

fn local_model(
    dir: &std::path::Path,
    base_url: &str,
    input: &str,
) -> (Arc<aether_providers::Store>, RunRequest) {
    let providers = Arc::new(aether_providers::Store::open(dir).unwrap());
    let saved = providers
        .save(
            serde_json::from_value(json!({"name":"Local", "endpoints":[{
                "slug":"main", "baseUrl":base_url, "protocol":"openai-chat", "models":[{
                    "id":"m1", "modelType":"chat", "typeSource":"user", "contextWindow":null,
                    "maxOutput":null, "canVision":true, "supportsTools":true
                }]
            }]}))
            .unwrap(),
        )
        .unwrap();
    let request = serde_json::from_value(json!({
        "purpose":"chat", "model":{"endpointId":saved[0].endpoints[0].id,"modelId":"m1"},
        "skills":[], "input":input, "regenerate":false, "language":"en"
    }))
    .unwrap();
    (providers, request)
}

fn service(
    host: Arc<dyn KitHost>,
    providers: Arc<aether_providers::Store>,
    store: Option<Arc<dyn ConversationStore>>,
) -> Arc<RunService> {
    let mut dependencies = RunDependencies::new(providers);
    dependencies.conversations = store;
    RunService::new(
        host,
        RunConfig::default(),
        dependencies,
        AddressPolicy::default(),
    )
    .unwrap()
}

fn context(owner: &RunOwner) -> RunContext {
    RunContext {
        owner: owner.clone(),
        own_mcp_port: None,
    }
}
fn events() -> (EventSink, mpsc::UnboundedReceiver<RunEvent>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        Arc::new(move |event| {
            sender.send(event).unwrap();
            Ok(())
        }),
        receiver,
    )
}
async fn until_end(receiver: &mut mpsc::UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        loop {
            let event = receiver.recv().await.unwrap();
            let ended = matches!(event, RunEvent::End { .. });
            events.push(event);
            if ended {
                return events;
            }
        }
    })
    .await
    .unwrap()
}
fn assert_end(events: &[RunEvent], expected: EndStatus) {
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::End { .. }))
            .count(),
        1
    );
    assert!(matches!(events.last(), Some(RunEvent::End { status, .. }) if *status == expected));
}
fn prepared(
    host: Arc<dyn KitHost>,
    providers: Arc<aether_providers::Store>,
    store: Option<Arc<dyn ConversationStore>>,
    request: RunRequest,
) -> Result<crate::prepare::Prepared, KitError> {
    let mut deps = RunDependencies::new(providers);
    deps.conversations = store;
    crate::prepare::prepare(
        host,
        RunConfig::default(),
        &deps,
        AddressPolicy::default(),
        request,
        "prepared-run".into(),
    )
}
fn reserve(
    store: &Arc<SqliteStore>,
    providers: Arc<aether_providers::Store>,
    request: RunRequest,
) -> TurnHandle {
    let preparation = prepared(Arc::new(Host), providers, Some(store.clone()), request).unwrap();
    let mut input = Some(preparation.input);
    store
        .reserve_action(&preparation.action, &mut |source| {
            let mut input = input.take().unwrap();
            crate::prepare::apply_history(&mut input, source);
            aether_chat::RunEngine::new(AddressPolicy::default())
                .unwrap()
                .validate(&mut input)?;
            Ok(input)
        })
        .unwrap()
        .handle
}
fn reply_record(model: &ModelRef, text: &str) -> TurnRecord {
    TurnRecord {
        status: EndStatus::Completed,
        messages: vec![Message::Assistant {
            text: text.into(),
            tool_calls: vec![],
            reasoning: vec![],
        }],
        model_id: Some(model.model_id.clone()),
        endpoint_id: Some(model.endpoint_id.clone()),
        skills: vec![],
        error_code: None,
        input_tokens: Some(10),
        output_tokens: Some(4),
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        context_usage: None,
        reasoning_effort: None,
        cost: None,
    }
}
fn part(data: &str) -> ImagePart {
    ImagePart {
        mime: "image/png".into(),
        data: data.into(),
    }
}

#[tokio::test]
async fn runs_without_history_and_keeps_a_bounded_terminal_snapshot() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "hi");
    let service = service(Arc::new(Host), providers, None);
    let owner = RunOwner::new();
    let (sink, mut receiver) = events();
    let accepted = service
        .start(context(&owner), request.clone(), sink)
        .await
        .unwrap();
    assert!(accepted.conversation_id.is_none());
    assert!(accepted.turn_id.is_none());
    let received = until_end(&mut receiver).await;
    assert_end(&received, EndStatus::Completed);
    let text: String = received
        .iter()
        .filter_map(|event| {
            if let RunEvent::TextDelta { text } = event {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, "Hello");
    let snapshot = service.snapshot(&owner, &accepted.run_id).unwrap();
    assert_eq!(snapshot.status, RunStatus::Completed);
    assert!(!snapshot.cancel_requested);
    let wire = serde_json::to_value(snapshot).unwrap();
    assert!(wire.get("input").is_none());
    assert!(wire.get("messages").is_none());
    assert!(wire.get("connection").is_none());
    assert!(service.cancel(&owner, &accepted.run_id).unwrap().ok);
    let (sink, mut rejected) = events();
    let error = service
        .start(
            context(&owner),
            RunRequest {
                regenerate: true,
                conversation_id: Some("c1".into()),
                ..request
            },
            sink,
        )
        .await
        .unwrap_err();
    assert_eq!(error.details().reason(), Some("no_conversation"));
    assert!(rejected.try_recv().is_err());
    service.shutdown(Duration::from_secs(2)).await;
    assert!(receiver.try_recv().is_err(), "no second End");
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("POST /v1/chat/completions"));
    assert!(requests[0].contains("\"content\":\"hi\""));
}

struct Notes {
    deny: bool,
    calls: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl KitHost for Notes {
    fn tools(&self, _: &RunRequest, _: &str) -> Vec<HostTool> {
        [
            ("read_note", Approval::Auto, ToolEffect::Read),
            ("write_note", Approval::Ask, ToolEffect::Edit),
        ]
        .map(|(name, approval, effect)| HostTool {
            spec: ToolSpec {
                name: name.into(),
                description: name.into(),
                parameters: json!({"type":"object","properties":{}}),
                approval,
                effect,
                source: ToolSource::Host,
            },
        })
        .into()
    }
    async fn call_tool(
        &self,
        call: &ToolCall,
        _: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.lock().unwrap().push(call.name.clone());
        Ok(ToolOutput {
            text: format!("{} done", call.name),
            payload: None,
        })
    }
    fn authorize(&self, call: &ToolCall, context: &AuthorizeContext<'_>) -> Approval {
        if self.deny && call.name == "write_note" {
            Approval::Deny
        } else {
            context.proposed
        }
    }
}

async fn tool_run(deny: bool, answer: Option<bool>) -> (Arc<Notes>, Vec<RunEvent>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let gate = Arc::new(Notify::new());
    let server_gate = gate.clone();
    let server = tokio::spawn(async move {
        server_gate.notified().await;
        upstream(listener, vec![TWO_TOOLS, HELLO]).await
    });
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "note");
    let host = Arc::new(Notes {
        deny,
        calls: Mutex::default(),
    });
    let service = service(host.clone(), providers, None);
    let owner = RunOwner::new();
    // The real synchronous sink answers before ApprovalRequired returns to the engine.
    let run_id = Arc::new(Mutex::new(None::<String>));
    let (sink, mut receiver) = events();
    let callback_service = Arc::downgrade(&service);
    let callback_owner = owner.clone();
    let callback_id = run_id.clone();
    let callback: EventSink = Arc::new(move |event| {
        if let RunEvent::ApprovalRequired { call_id, .. } = &event {
            let service = callback_service.upgrade().unwrap();
            let run_id = callback_id.lock().unwrap().clone().unwrap();
            let other = RunOwner::new();
            assert_eq!(
                service
                    .approve(&other, &run_id, call_id, true)
                    .unwrap_err()
                    .code(),
                KitErrorCode::NotFound
            );
            assert!(
                service
                    .approve(
                        &callback_owner,
                        &run_id,
                        call_id,
                        answer.expect("unexpected approval")
                    )
                    .unwrap()
                    .ok
            );
            assert_eq!(
                service
                    .approve(&callback_owner, &run_id, call_id, true)
                    .unwrap_err()
                    .code(),
                KitErrorCode::Conflict
            );
        }
        sink(event)
    });
    let accepted = service
        .start(context(&owner), request, callback)
        .await
        .unwrap();
    *run_id.lock().unwrap() = Some(accepted.run_id.clone());
    gate.notify_one();
    let received = until_end(&mut receiver).await;
    assert_end(&received, EndStatus::Completed);
    assert_eq!(
        service
            .approve(&owner, &accepted.run_id, "c2", true)
            .unwrap_err()
            .code(),
        KitErrorCode::Conflict
    );
    service.shutdown(Duration::from_secs(2)).await;
    let requests = server.await.unwrap();
    (host, received, requests[1].clone())
}

#[tokio::test]
async fn host_tools_execute_auto_and_immediately_approved_calls() {
    let (host, events, second) = tool_run(false, Some(true)).await;
    assert_eq!(*host.calls.lock().unwrap(), ["read_note", "write_note"]);
    let outcomes: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let RunEvent::ToolResult {
                call_id, outcome, ..
            } = event
            {
                Some((call_id.as_str(), *outcome))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(outcomes, [("c1", ToolOutcome::Ok), ("c2", ToolOutcome::Ok)]);
    assert!(second.contains("write_note done"));
}

#[tokio::test]
async fn user_and_host_denials_never_execute() {
    for (deny, answer) in [(false, Some(false)), (true, None)] {
        let (host, events, second) = tool_run(deny, answer).await;
        assert_eq!(*host.calls.lock().unwrap(), ["read_note"]);
        assert!(events.iter().any(|event| matches!(event, RunEvent::ToolResult { call_id, outcome: ToolOutcome::Denied, .. } if call_id == "c2")));
        if deny {
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, RunEvent::ApprovalRequired { .. }))
            );
        }
        assert!(second.contains("该工具调用已被拒绝，未执行。"));
    }
}

#[tokio::test]
async fn approval_timeout_cancel_and_consumption_remove_pending_entries() {
    let approvals = crate::tools::Approvals::new(Duration::from_millis(10));
    let call = ToolCall {
        id: "c1".into(),
        name: "write_note".into(),
        args: json!({}),
    };
    let cancel = CancellationToken::new();
    assert!(!approvals.wait("r1", &call, &cancel).await);
    assert!(!approvals.answer("r1", "c1", true));
    let waiting = approvals.wait("r1", &call, &cancel);
    cancel.cancel();
    assert!(!waiting.await);
    assert!(!approvals.answer("r1", "c1", true));
    let cancel = CancellationToken::new();
    let waiting = approvals.wait("r2", &call, &cancel);
    assert!(approvals.answer("r2", "c1", true));
    assert!(waiting.await);
    assert!(!approvals.answer("r2", "c1", false));
}

#[tokio::test]
async fn owners_cannot_observe_approve_or_cancel_each_others_active_runs() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![TWO_TOOLS]));
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "note");
    let host = Arc::new(Notes {
        deny: false,
        calls: Mutex::default(),
    });
    let service = service(host.clone(), providers, None);
    let owner = RunOwner::new();
    let other = RunOwner::new();
    let (sink, mut receiver) = events();
    let accepted = service
        .start(context(&owner), request.clone(), sink)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(
            receiver.recv().await.unwrap(),
            RunEvent::ApprovalRequired { .. }
        ) {}
    })
    .await
    .unwrap();
    assert_eq!(
        service.snapshot(&owner, &accepted.run_id).unwrap().status,
        RunStatus::Running
    );
    assert_eq!(
        service
            .snapshot(&other, &accepted.run_id)
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service.snapshot(&owner, "unknown").unwrap_err().code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service
            .approve(&other, &accepted.run_id, "c2", true)
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service
            .approve(&owner, "unknown", "c2", true)
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    assert!(service.cancel(&other, &accepted.run_id).unwrap().ok);
    assert!(service.cancel(&owner, "unknown").unwrap().ok);
    assert!(
        !service
            .snapshot(&owner, &accepted.run_id)
            .unwrap()
            .cancel_requested
    );
    service.cancel_owner(&other);
    assert!(
        !service
            .snapshot(&owner, &accepted.run_id)
            .unwrap()
            .cancel_requested
    );
    service.cancel_owner(&owner.clone());
    assert!(
        service
            .snapshot(&owner, &accepted.run_id)
            .unwrap()
            .cancel_requested
    );
    assert_eq!(
        service
            .approve(&owner, &accepted.run_id, "c2", true)
            .unwrap_err()
            .code(),
        KitErrorCode::Conflict
    );
    let received = until_end(&mut receiver).await;
    assert_end(&received, EndStatus::Cancelled);
    assert_eq!(
        service.snapshot(&owner, &accepted.run_id).unwrap().status,
        RunStatus::Cancelled
    );
    assert_eq!(*host.calls.lock().unwrap(), ["read_note"]);
    let (sink, mut rejected) = events();
    assert_eq!(
        service
            .start(context(&owner), request, sink)
            .await
            .unwrap_err()
            .code(),
        KitErrorCode::Cancelled
    );
    assert!(rejected.try_recv().is_err());
    service.shutdown(Duration::from_secs(2)).await;
    server.await.unwrap();
}

struct AllModes;
impl KitHost for AllModes {
    fn permission_modes(&self, _: &RunRequest) -> Vec<PermissionMode> {
        vec![
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
            PermissionMode::FullAccess,
        ]
    }
    fn mcp_tools(&self, request: &RunRequest) -> bool {
        request.purpose != "writing"
    }
}

#[test]
fn permission_modes_require_host_consent_and_keep_wire_names() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), "http://localhost/v1", "hi");
    assert_eq!(
        prepared(Arc::new(Host), providers.clone(), None, request.clone())
            .unwrap()
            .input
            .permission_mode,
        PermissionMode::Default
    );
    for mode in PermissionMode::BASELINE {
        let request = RunRequest {
            permission_mode: Some(mode),
            ..request.clone()
        };
        assert_eq!(
            prepared(Arc::new(Host), providers.clone(), None, request)
                .unwrap()
                .input
                .permission_mode,
            mode
        );
    }
    for mode in [PermissionMode::Auto, PermissionMode::FullAccess] {
        let request = RunRequest {
            permission_mode: Some(mode),
            ..request.clone()
        };
        let error = prepared(Arc::new(Host), providers.clone(), None, request.clone())
            .err()
            .unwrap();
        assert_eq!(error.code(), KitErrorCode::BadRequest);
        assert_eq!(error.details().field(), Some("permission_mode"));
        assert_eq!(error.details().reason(), Some("not_allowed"));
        assert_eq!(
            prepared(Arc::new(AllModes), providers.clone(), None, request)
                .unwrap()
                .input
                .permission_mode,
            mode
        );
    }
    let parsed: RunRequest = serde_json::from_value(json!({"purpose":"chat","skills":[],"input":"hi","regenerate":false,"language":"en","permissionMode":"accept_edits"})).unwrap();
    assert_eq!(parsed.permission_mode, Some(PermissionMode::AcceptEdits));
}

#[test]
fn actions_validate_shape_language_images_and_store_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut action) = local_model(dir.path(), "http://localhost/v1", "");
    action.conversation_id = Some("conversation".into());
    action.retry_turn_id = Some("source".into());
    crate::prepare::check_request(&action, true).unwrap();
    assert!(crate::prepare::check_request(&action, false).is_err());
    action.regenerate = true;
    assert!(crate::prepare::check_request(&action, true).is_err());
    action.regenerate = false;
    action.translate_turn_id = Some("source".into());
    action.target_language = Some("French".into());
    assert!(crate::prepare::check_request(&action, true).is_err());
    action.retry_turn_id = None;
    crate::prepare::check_request(&action, true).unwrap();
    for language in [" ".into(), "x".repeat(81)] {
        action.target_language = Some(language);
        assert!(crate::prepare::check_request(&action, true).is_err());
    }
    action.target_language = Some("French".into());
    action.images = vec![part("aW4x")];
    let error = crate::prepare::check_request(&action, true).unwrap_err();
    assert_eq!(error.details().reason(), Some("action_conflict"));
}
