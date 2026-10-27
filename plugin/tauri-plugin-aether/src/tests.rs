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

use aether_chat::{
    Approver, EndStatus, PermissionMode, RunEvent, Runner, ToolEffect, ToolOutcome, ToolSource,
    ToolSpec,
};
use aether_observability::{LogConfig, LogHandle};
use aether_support::KitErrorCode;
use aether_support::net::AddressPolicy;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::tools::{Approvals, HostTools};
use crate::{
    Approval, AuthorizeContext, Builder, CancellationToken, ChatConfig, HostTool, KitHost,
    RunRequest, ToolCall, ToolError, ToolOutput,
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
                "slug": "main", "baseUrl": base_url, "protocol": "openai-chat",
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
    .unwrap()
    .0;
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
    fn tools(&self, _req: &RunRequest, _run_id: &str) -> Vec<HostTool> {
        [
            ("read_note", Approval::Auto, ToolEffect::Read),
            ("write_note", Approval::Ask, ToolEffect::Edit),
        ]
        .map(|(name, approval, effect)| HostTool {
            spec: ToolSpec {
                name: name.to_owned(),
                description: format!("{name} tool"),
                parameters: json!({ "type": "object", "properties": {} }),
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
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.lock().unwrap().push(call.name.clone());
        Ok(ToolOutput {
            text: format!("{} done", call.name),
            payload: None,
        })
    }
    fn authorize(&self, call: &ToolCall, ctx: &AuthorizeContext<'_>) -> Approval {
        if self.deny && call.name == "write_note" {
            Approval::Deny
        } else {
            ctx.proposed
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
    .unwrap()
    .0;
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

fn store_reply(
    store: &aether_chat::SqliteStore,
    turn: aether_chat::TurnHandle,
    model: &crate::ModelRef,
    text: &str,
) {
    use aether_chat::ConversationStore;
    store
        .finish_turn(
            turn,
            aether_chat::TurnRecord {
                status: EndStatus::Completed,
                messages: vec![aether_chat::Message::Assistant {
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
            },
        )
        .unwrap();
}

#[test]
fn actions_validate_shape_language_and_store_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let (_, request) = local_model(dir.path(), "http://localhost/v1", "hi");
    let mut action = request;
    action.input.clear();
    action.conversation_id = Some("conversation".into());
    action.retry_turn_id = Some("source".into());
    super::chat::check_request(&action, true).unwrap();
    assert!(super::chat::check_request(&action, false).is_err());
    action.regenerate = true;
    assert!(super::chat::check_request(&action, true).is_err());
    action.regenerate = false;
    action.translate_turn_id = Some("source".into());
    action.target_language = Some("French".into());
    assert!(super::chat::check_request(&action, true).is_err());
    action.retry_turn_id = None;
    super::chat::check_request(&action, true).unwrap();
    action.target_language = Some(" ".into());
    assert!(super::chat::check_request(&action, true).is_err());
    action.target_language = Some("x".repeat(81));
    assert!(super::chat::check_request(&action, true).is_err());
}

/// Offers every mode, and MCP tools only outside `writing`.
struct AllModes;
impl KitHost for AllModes {
    fn permission_modes(&self, _req: &RunRequest) -> Vec<PermissionMode> {
        vec![
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
            PermissionMode::FullAccess,
        ]
    }
    fn mcp_tools(&self, req: &RunRequest) -> bool {
        req.purpose != "writing"
    }
}

#[test]
fn permission_mode_must_be_offered_by_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), "http://localhost/v1", "hi");
    let assemble = |host: &dyn KitHost, mode: Option<PermissionMode>| {
        let request = RunRequest {
            permission_mode: mode,
            ..request.clone()
        };
        let budget = ChatConfig::default().budget;
        super::chat::assemble(
            host,
            &providers,
            None,
            Vec::new(),
            budget,
            &request,
            "r1".into(),
        )
        .map(|(input, _)| input.permission_mode)
    };
    // Omitted is `Default`; the baseline needs no host code.
    assert_eq!(assemble(&Host, None).unwrap(), PermissionMode::Default);
    for mode in PermissionMode::BASELINE {
        assert_eq!(assemble(&Host, Some(mode)).unwrap(), mode);
    }
    for mode in [PermissionMode::Auto, PermissionMode::FullAccess] {
        let error = assemble(&Host, Some(mode)).unwrap_err();
        assert_eq!(error.code(), KitErrorCode::BadRequest);
        assert_eq!(error.details().field(), Some("permission_mode"));
        assert_eq!(error.details().reason(), Some("not_allowed"));
        assert_eq!(assemble(&AllModes, Some(mode)).unwrap(), mode);
    }
    // The wire name is snake_case.
    let parsed: RunRequest = serde_json::from_value(json!({
        "purpose": "chat", "skills": [], "input": "hi", "regenerate": false, "language": "en",
        "permissionMode": "accept_edits"
    }))
    .unwrap();
    assert_eq!(parsed.permission_mode, Some(PermissionMode::AcceptEdits));
}

#[derive(Default)]
struct RunBoundTools {
    snapshots: Mutex<std::collections::HashMap<String, String>>,
    finished: Mutex<Vec<String>>,
}

impl KitHost for RunBoundTools {
    fn tools(&self, req: &RunRequest, run_id: &str) -> Vec<HostTool> {
        self.snapshots
            .lock()
            .unwrap()
            .insert(run_id.to_owned(), req.input.clone());
        vec![HostTool {
            spec: ToolSpec {
                name: "pinned".into(),
                description: "Pinned run snapshot".into(),
                parameters: json!({ "type": "object", "properties": {} }),
                approval: Approval::Ask,
                effect: ToolEffect::Exec,
                source: ToolSource::Host,
            },
        }]
    }

    fn authorize(&self, call: &ToolCall, ctx: &AuthorizeContext<'_>) -> Approval {
        if self
            .snapshots
            .lock()
            .unwrap()
            .get(ctx.run_id)
            .is_some_and(|snapshot| {
                call.args
                    .get("snapshot")
                    .and_then(serde_json::Value::as_str)
                    == Some(snapshot.as_str())
            })
        {
            ctx.proposed
        } else {
            Approval::Deny
        }
    }

    fn run_finished(&self, run_id: &str) {
        self.snapshots.lock().unwrap().remove(run_id);
        self.finished.lock().unwrap().push(run_id.to_owned());
    }
}

#[test]
fn host_tool_snapshots_are_run_bound_and_released_independently() {
    let host = Arc::new(RunBoundTools::default());
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), "http://localhost/v1", "revision-one");
    let assemble = |request: &RunRequest, id: &str| {
        super::chat::assemble(
            host.as_ref(),
            &providers,
            None,
            Vec::new(),
            ChatConfig::default().budget,
            request,
            id.into(),
        )
        .unwrap()
        .0
    };
    let first = super::chat::HostRun {
        host: host.clone(),
        run_id: "first".into(),
    };
    let one = assemble(&request, "first");
    request.input = "revision-two".into();
    let second = super::chat::HostRun {
        host: host.clone(),
        run_id: "second".into(),
    };
    let two = assemble(&request, "second");
    let authorize = |input: &aether_chat::RunInput, snapshot: &str| {
        let call = ToolCall {
            id: "call".into(),
            name: "pinned".into(),
            args: json!({ "snapshot": snapshot }),
        };
        host.authorize(
            &call,
            &AuthorizeContext {
                mode: input.permission_mode,
                purpose: &input.purpose,
                run_id: &input.run_id,
                effect: ToolEffect::Exec,
                source: &ToolSource::Host,
                proposed: Approval::Ask,
            },
        )
    };
    assert_eq!(authorize(&one, "revision-one"), Approval::Ask);
    assert_eq!(authorize(&two, "revision-two"), Approval::Ask);
    assert_eq!(authorize(&two, "revision-one"), Approval::Deny);
    drop(first);
    assert_eq!(authorize(&one, "revision-one"), Approval::Deny);
    assert_eq!(authorize(&two, "revision-two"), Approval::Ask);
    drop(second);
    assert!(host.snapshots.lock().unwrap().is_empty());
    assert_eq!(*host.finished.lock().unwrap(), ["first", "second"]);
}

#[test]
fn host_cleanup_covers_rejected_assembly() {
    let host = Arc::new(RunBoundTools::default());
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), "http://localhost/v1", "revision-one");
    request.permission_mode = Some(PermissionMode::FullAccess);
    let rejected = || {
        let _guard = super::chat::HostRun {
            host: host.clone(),
            run_id: "rejected".into(),
        };
        super::chat::assemble(
            host.as_ref(),
            &providers,
            None,
            Vec::new(),
            ChatConfig::default().budget,
            &request,
            "rejected".into(),
        )
    };
    assert!(rejected().is_err());
    assert!(host.snapshots.lock().unwrap().is_empty());
    assert_eq!(*host.finished.lock().unwrap(), ["rejected"]);
}

#[cfg(feature = "mcp-client")]
#[tokio::test]
async fn mcp_tools_false_opens_no_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_, request) = local_model(dir.path(), "http://localhost/v1", "hi");
    let writing = RunRequest {
        purpose: "writing".to_owned(),
        ..request.clone()
    };
    let store = aether_mcp_client::Store::open(dir.path()).unwrap();
    let cancel = CancellationToken::new();
    let opened = std::sync::atomic::AtomicU8::new(0);
    let (store, cancel, opened) = (&store, &cancel, &opened);
    let open = move || async move {
        opened.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (Vec::new(), store.session(cancel, None).await)
    };
    let host = || HostTools::new(Arc::new(AllModes));

    let mut tools = Vec::new();
    let executor = super::tools::with_mcp(host(), &mut tools, &writing, open).await;
    assert_eq!(opened.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(executor.mcp.is_none());
    assert!(tools.is_empty());

    let executor = super::tools::with_mcp(host(), &mut tools, &request, open).await;
    assert_eq!(opened.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(executor.mcp.is_some());
}

#[tokio::test]
async fn retry_and_translation_use_source_ancestry_and_real_runner() {
    use aether_chat::{ConversationStore, Message, SqliteStore, SystemPart};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), &url, "q1");
    let model = request.model.clone().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = store.begin_turn(None, Some("note:1"), "q1").unwrap();
    let conversation_id = first.conversation_id.clone();
    let first_id = first.turn_id.clone();
    store_reply(&store, first, &model, "original answer");
    let candidate = store
        .begin_action(
            Some(&conversation_id),
            None,
            "ignored",
            &[],
            None,
            Some(&first_id),
            None,
            None,
        )
        .unwrap();
    let candidate_id = candidate.turn_id.clone();
    store_reply(&store, candidate, &model, "alternative answer");
    let second = store
        .begin_turn(Some(&conversation_id), None, "q2")
        .unwrap();
    let second_id = second.turn_id.clone();
    store_reply(&store, second, &model, "second answer");
    request.conversation_id = Some(conversation_id.clone());
    request.input = "q3".into();
    let assemble = |request: &RunRequest| {
        super::chat::assemble(
            &Host,
            &providers,
            Some(&store),
            vec![SystemPart::Host("safety rules".into())],
            ChatConfig::default().budget,
            request,
            "r1".into(),
        )
        .map(|(input, _)| input)
    };
    let normal = assemble(&request).unwrap();
    assert!(normal.history.contains(&Message::User("q2".into())));
    assert!(
        !serde_json::to_string(&normal.history)
            .unwrap()
            .contains("alternative answer")
    );
    request.model = None;
    request.retry_turn_id = Some(second_id);
    let retry = assemble(&request).unwrap();
    assert_eq!(retry.input, "q2");
    assert_eq!(
        retry.connection.endpoint_id.as_deref(),
        Some(model.endpoint_id.as_str())
    );
    assert_eq!(retry.history.len(), 2);
    assert!(retry.history.contains(&Message::User("q1".into())));
    assert!(matches!(&retry.system[0], SystemPart::Host(text) if text == "safety rules"));
    request.retry_turn_id = None;
    request.regenerate = true;
    let regenerated = assemble(&request).unwrap();
    assert_eq!(regenerated.input, "q2");
    assert_eq!(regenerated.history, retry.history);
    request.regenerate = false;
    request.retry_turn_id = None;
    request.translate_turn_id = Some(candidate_id);
    request.target_language = Some("French".into());
    request.model = None;
    let translation = assemble(&request).unwrap();
    assert_eq!(
        translation.connection.endpoint_id.as_deref(),
        Some(model.endpoint_id.as_str())
    );
    assert_eq!(
        translation.input,
        "Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\nalternative answer"
    );
    assert_eq!(translation.history.len(), 2);
    assert!(
        !serde_json::to_string(&translation.history)
            .unwrap()
            .contains("second answer")
    );
    let summary = Runner::new(AddressPolicy::default())
        .unwrap()
        .run(
            translation,
            &HostTools::new(Arc::new(Host)),
            &Approvals::new(Duration::from_secs(1)),
            &CancellationToken::new(),
            |_| {},
        )
        .await;
    assert_eq!(summary.status, EndStatus::Completed);
    let received = server.await.unwrap();
    assert!(received[0].contains("Translate the following reply into French"));
    assert!(received[0].contains("safety rules"));
    request.translate_turn_id = Some("foreign".into());
    assert_eq!(
        assemble(&request).unwrap_err().code(),
        KitErrorCode::NotFound
    );
    request.translate_turn_id = None;
    request.target_language = None;
    request.retry_turn_id = Some(first_id);
    request.model = None;
    let provider_id = providers.list().unwrap()[0].id.clone();
    providers.delete(&provider_id).unwrap();
    assert_eq!(
        assemble(&request).unwrap_err().code(),
        KitErrorCode::NeedsConfig
    );
    request.model =
        Some(serde_json::from_value(json!({"endpointId": "missing", "modelId": "m"})).unwrap());
    let running = store
        .begin_turn(Some(&conversation_id), None, "running")
        .unwrap();
    assert_eq!(
        assemble(&request).unwrap_err().details().reason(),
        Some("turn_running")
    );
    store_reply(&store, running, request.model.as_ref().unwrap(), "done");
}

#[tokio::test]
async fn end_is_returned_only_after_durable_finish_and_storage_failure_is_explicit() {
    use aether_chat::{ConversationStore, Message, SqliteStore, TurnRecord, TurnStatus};
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let turn = store.begin_turn(None, None, "question").unwrap();
    let id = turn.conversation_id.clone();
    let record = TurnRecord {
        status: EndStatus::Completed,
        messages: vec![Message::Assistant {
            text: "answer".into(),
            tool_calls: vec![],
            reasoning: vec![
                aether_support::ReasoningBlock {
                    protocol: aether_support::Protocol::AnthropicMessages,
                    endpoint_id: Some("e1".into()),
                    model_id: Some("m1".into()),
                    value: json!({"type":"thinking","thinking":"visible thought", "signature":"private signature"}),
                },
                aether_support::ReasoningBlock {
                    protocol: aether_support::Protocol::AnthropicMessages,
                    endpoint_id: Some("e1".into()),
                    model_id: Some("m1".into()),
                    value: json!({"type":"redacted_thinking","data":"private data"}),
                },
                aether_support::ReasoningBlock {
                    protocol: aether_support::Protocol::OpenaiResponses,
                    endpoint_id: Some("e1".into()),
                    model_id: Some("m1".into()),
                    value: json!({"type":"reasoning","summary":[{"type":"summary_text","text":"visible summary"}],"encrypted_content":"private encrypted"}),
                },
                aether_support::ReasoningBlock {
                    protocol: aether_support::Protocol::OpenaiChat,
                    endpoint_id: Some("e1".into()),
                    model_id: Some("m1".into()),
                    value: json!("visible chat"),
                },
            ],
        }],
        model_id: Some("m1".into()),
        endpoint_id: Some("e1".into()),
        skills: vec![],
        error_code: None,
        input_tokens: Some(1),
        output_tokens: Some(2),
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        context_usage: None,
        reasoning_effort: None,
        cost: Some(aether_support::Cost {
            amount: 0.125,
            currency: aether_support::Currency::Usd,
        }),
    };
    let end = || RunEvent::End {
        status: EndStatus::Completed,
        error: None,
        exhausted: None,
    };
    let event = super::chat::finish_event(store.clone(), turn.clone(), record.clone(), end()).await;
    assert!(matches!(
        event,
        RunEvent::End {
            status: EndStatus::Completed,
            ..
        }
    ));
    assert_eq!(
        store.load(&id).unwrap().turns[0].status,
        TurnStatus::Completed
    );
    let wire =
        serde_json::to_value(super::chat::Conversation::from(store.load(&id).unwrap())).unwrap();
    assert_eq!(wire["summary"]["selectedTurnId"], turn.turn_id);
    assert_eq!(wire["turns"][0]["questionId"], turn.turn_id);
    assert!(wire["turns"][0]["parentId"].is_null());
    assert!(wire["turns"][0]["supersedesId"].is_null());
    assert_eq!(wire["turns"][0]["kind"], "reply");
    assert_eq!(wire["turns"][0]["endpointId"], "e1");
    assert_eq!(
        wire["turns"][0]["cost"],
        json!({"amount": 0.125, "currency": "USD"})
    );
    assert_eq!(
        wire["turns"][0]["reasoningText"],
        "visible thoughtvisible summaryvisible chat"
    );
    assert!(!wire.to_string().contains("private"));
    let accepted = serde_json::to_value(crate::RunAccepted {
        run_id: "r1".into(),
        conversation_id: Some(id.clone()),
        turn_id: Some(turn.turn_id.clone()),
    })
    .unwrap();
    assert_eq!(accepted["turnId"], turn.turn_id);
    let failure = super::chat::finish_event(store, turn, record, end()).await;
    assert!(matches!(
        failure,
        RunEvent::End {
            status: EndStatus::Failed,
            error: Some(_),
            ..
        }
    ));
}

#[test]
fn earlier_generated_image_is_selectable_and_ids_are_checked() {
    use crate::tools::{ImageChoice, image_choices, painting_spec, resolve_images};
    use aether_chat::{ConversationStore, ImagePart, SqliteStore};
    let part = |data: &str| ImagePart {
        mime: "image/png".into(),
        data: data.into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), "http://127.0.0.1:9/v1", "q2");
    let model = request.model.clone().unwrap();
    let sqlite = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let store: Arc<dyn ConversationStore> = sqlite.clone();
    // Turn 1: the user attached `in1`; the reply generated `gen1`.
    let first = store
        .begin_action(
            None,
            Some("note:1"),
            "draw",
            &[part("aW4x")],
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let conversation_id = first.conversation_id.clone();
    store
        .add_generated(&first.turn_id, &[part("Z2VuMQ==")])
        .unwrap();
    store_reply(&sqlite, first, &model, "done");
    request.conversation_id = Some(conversation_id.clone());
    request.images = vec![part("bm93")];
    let (input, path) = super::chat::assemble(
        &Host,
        &providers,
        Some(store.as_ref()),
        Vec::new(),
        ChatConfig::default().budget,
        &request,
        "r1".into(),
    )
    .unwrap();
    let choices = image_choices(&input.images, &path);
    assert_eq!(choices.len(), 3);
    assert_eq!(choices[0], ImageChoice::Input(part("bm93")));
    assert!(matches!(
        choices[1],
        ImageChoice::Stored {
            generated: true,
            idx: 0,
            ..
        }
    ));
    assert!(matches!(
        choices[2],
        ImageChoice::Stored {
            generated: false,
            idx: 0,
            ..
        }
    ));

    let mut painting = crate::tools::Painting {
        connection: input.connection,
        policy: AddressPolicy::default(),
        store: None,
        calls: std::sync::atomic::AtomicU8::new(0),
        choices: choices.into(),
        history: Some((Arc::clone(&store), conversation_id)),
    };
    // Not an edit model: no `image_ids`, and passing one is rejected.
    assert!(
        painting_spec(&painting).parameters["properties"]
            .get("image_ids")
            .is_none()
    );
    let resolve = |painting: &crate::tools::Painting, args| {
        resolve_images(
            &painting.choices,
            painting.history.as_ref(),
            painting.connection.can_vision,
            &args,
        )
    };
    let reason = |result: Result<Vec<ImagePart>, aether_support::KitError>| {
        result.unwrap_err().details().reason().map(str::to_owned)
    };
    assert_eq!(
        reason(resolve(&painting, json!({"image_ids": ["image-2"]}))).as_deref(),
        Some("unsupported")
    );
    painting.connection.can_vision = true;
    let spec = painting_spec(&painting);
    assert_eq!(
        spec.parameters["properties"]["image_ids"]["items"]["enum"],
        json!(["image-1", "image-2", "image-3"])
    );
    assert!(
        spec.description
            .contains("image-2: generated in an earlier reply")
    );
    assert!(
        spec.description
            .contains("image-3: attached in an earlier message")
    );
    assert_eq!(
        resolve(&painting, json!({"image_ids": ["image-2", "image-1"]})).unwrap(),
        vec![part("Z2VuMQ=="), part("bm93")]
    );
    assert!(
        resolve(&painting, json!({"prompt": "x"}))
            .unwrap()
            .is_empty()
    );
    for (args, want) in [
        (json!({"image_ids": ["image-9"]}), "unknown"),
        (json!({"image_ids": ["x"]}), "unknown"),
        (json!({"image_ids": vec!["image-1"; 5]}), "too_many"),
    ] {
        assert_eq!(reason(resolve(&painting, args)).as_deref(), Some(want));
    }
    // Purged bytes: the choice still exists but is expired.
    store.purge_images(i64::MAX).unwrap();
    assert_eq!(
        reason(resolve(&painting, json!({"image_ids": ["image-2"]}))).as_deref(),
        Some("expired")
    );
}
