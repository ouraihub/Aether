use super::*;
use std::collections::HashMap;

#[derive(Default)]
struct PinnedHost {
    snapshots: Mutex<HashMap<String, String>>,
    finished: Mutex<Vec<String>>,
    released: Notify,
}
impl KitHost for PinnedHost {
    fn tools(&self, request: &RunRequest, run_id: &str) -> Vec<HostTool> {
        self.snapshots
            .lock()
            .unwrap()
            .insert(run_id.into(), request.input.clone());
        vec![HostTool {
            spec: ToolSpec {
                name: "pinned".into(),
                description: "Pinned capability".into(),
                parameters: json!({"type":"object","properties":{}}),
                approval: Approval::Ask,
                effect: ToolEffect::Exec,
                source: ToolSource::Host,
            },
        }]
    }
    fn authorize(&self, call: &ToolCall, context: &AuthorizeContext<'_>) -> Approval {
        if self
            .snapshots
            .lock()
            .unwrap()
            .get(context.run_id)
            .is_some_and(|input| call.args["snapshot"].as_str() == Some(input.as_str()))
        {
            context.proposed
        } else {
            Approval::Deny
        }
    }
    fn run_finished(&self, id: &str) {
        self.snapshots.lock().unwrap().remove(id);
        self.finished.lock().unwrap().push(id.into());
        self.released.notify_one();
    }
}
impl PinnedHost {
    async fn released(&self, id: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self
                .finished
                .lock()
                .unwrap()
                .iter()
                .any(|finished| finished == id)
            {
                self.released.notified().await;
            }
        })
        .await
        .unwrap();
    }
    fn approval(&self, run_id: &str, value: &str) -> Approval {
        self.authorize(
            &ToolCall {
                id: "c".into(),
                name: "pinned".into(),
                args: json!({"snapshot":value}),
            },
            &AuthorizeContext {
                mode: PermissionMode::Default,
                purpose: "chat",
                run_id,
                effect: ToolEffect::Exec,
                source: &ToolSource::Host,
                proposed: Approval::Ask,
            },
        )
    }
}

#[tokio::test]
async fn run_bound_capabilities_release_independently_and_exactly_once() {
    // Keep both model requests pending; cancellation, not a fake engine, ends them.
    let server = upstream(vec![Reply::silent(), Reply::silent()]).await;
    let url = format!("{}/v1", server.url());
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "revision-one");
    let host = Arc::new(PinnedHost::default());
    let service = service(host.clone(), providers, None);
    let owner = RunOwner::new();
    let (sink, mut first_events) = events();
    let first = service
        .start(context(&owner), request.clone(), sink)
        .await
        .unwrap();
    let (sink, mut second_events) = events();
    let second = service
        .start(
            context(&owner),
            RunRequest {
                input: "revision-two".into(),
                ..request
            },
            sink,
        )
        .await
        .unwrap();
    assert_eq!(host.approval(&first.run_id, "revision-one"), Approval::Ask);
    assert_eq!(host.approval(&second.run_id, "revision-two"), Approval::Ask);
    assert_eq!(
        host.approval(&second.run_id, "revision-one"),
        Approval::Deny
    );
    service.cancel(&owner, &first.run_id).unwrap();
    assert_end(&until_end(&mut first_events).await, EndStatus::Cancelled);
    host.released(&first.run_id).await;
    assert_eq!(host.approval(&first.run_id, "revision-one"), Approval::Deny);
    assert_eq!(host.approval(&second.run_id, "revision-two"), Approval::Ask);
    service.cancel(&owner, &second.run_id).unwrap();
    assert_end(&until_end(&mut second_events).await, EndStatus::Cancelled);
    service.shutdown(Duration::from_secs(2)).await;
    assert!(host.snapshots.lock().unwrap().is_empty());
    let finished = host.finished.lock().unwrap();
    assert_eq!(finished.iter().filter(|id| **id == first.run_id).count(), 1);
    assert_eq!(
        finished.iter().filter(|id| **id == second.run_id).count(),
        1
    );
    drop(server);
}

#[tokio::test]
async fn rejected_preparation_cleans_host_once_without_events_or_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), "http://127.0.0.1:9/v1", "bad mode");
    request.permission_mode = Some(PermissionMode::FullAccess);
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let host = Arc::new(PinnedHost::default());
    let service = service(host.clone(), providers, Some(store.clone()));
    let (sink, mut receiver) = events();
    let error = service
        .start(context(&RunOwner::new()), request, sink)
        .await
        .unwrap_err();
    assert_eq!(error.details().reason(), Some("not_allowed"));
    service.shutdown(Duration::from_secs(2)).await;
    assert_eq!(host.finished.lock().unwrap().len(), 1);
    assert!(host.snapshots.lock().unwrap().is_empty());
    assert!(store.list(None, 200).unwrap().is_empty());
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn durable_finish_precedes_the_only_end_and_terminal_snapshot() {
    let server = upstream(vec![sse(HELLO)]).await;
    let url = format!("{}/v1", server.url());
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "question");
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let host = Arc::new(PinnedHost::default());
    let service = service(host.clone(), providers, Some(store.clone()));
    let owner = RunOwner::new();
    let observed = Arc::new(Mutex::new(None));
    let seen = observed.clone();
    let database = store.clone();
    let (sink, mut receiver) = events();
    let sink: EventSink = Arc::new(move |event| {
        if matches!(event, RunEvent::End { .. }) {
            let id = database.list(None, 1).unwrap()[0].id.clone();
            *seen.lock().unwrap() = Some(database.load(&id).unwrap());
        }
        sink(event)
    });
    let accepted = service.start(context(&owner), request, sink).await.unwrap();
    assert_end(&until_end(&mut receiver).await, EndStatus::Completed);
    let snapshot = service.snapshot(&owner, &accepted.run_id).unwrap();
    assert_eq!(snapshot.status, RunStatus::Completed);
    service.shutdown(Duration::from_secs(2)).await;
    assert!(receiver.try_recv().is_err());
    let saved = observed.lock().unwrap().take().unwrap();
    assert_eq!(saved.turns[0].status, TurnStatus::Completed);
    assert_eq!(saved.turns[0].id, accepted.turn_id.unwrap());
    assert_eq!(saved.summary.id, accepted.conversation_id.unwrap());
    assert!(
        serde_json::to_string(&saved.turns[0].messages)
            .unwrap()
            .contains("Hello")
    );
    assert_eq!(*host.finished.lock().unwrap(), [accepted.run_id]);
    wire_requests(&server, 1);
}

#[tokio::test]
async fn save_failure_replaces_completed_end_and_snapshot_without_reexecution() {
    let server = upstream(vec![sse(HELLO)]).await;
    let url = format!("{}/v1", server.url());
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "question");
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let host = Arc::new(PinnedHost::default());
    let service = service(host.clone(), providers, Some(store.clone()));
    let owner = RunOwner::new();
    let deleted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let database = store.clone();
    let once = deleted.clone();
    let (sink, mut receiver) = events();
    let sink: EventSink = Arc::new(move |event| {
        // Remove the real reserved row after output but before finalization.
        if matches!(event, RunEvent::TextDelta { .. })
            && !once.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            database
                .delete(&database.list(None, 1).unwrap()[0].id)
                .unwrap();
        }
        sink(event)
    });
    let accepted = service.start(context(&owner), request, sink).await.unwrap();
    let received = until_end(&mut receiver).await;
    assert_end(&received, EndStatus::Failed);
    assert!(
        matches!(received.last(), Some(RunEvent::End { error: Some(error), .. }) if error.code() == KitErrorCode::NotFound)
    );
    let snapshot = service.snapshot(&owner, &accepted.run_id).unwrap();
    assert_eq!(snapshot.status, RunStatus::Failed);
    assert_eq!(snapshot.error.unwrap().code(), KitErrorCode::NotFound);
    service.shutdown(Duration::from_secs(2)).await;
    assert!(receiver.try_recv().is_err());
    assert!(deleted.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(*host.finished.lock().unwrap(), [accepted.run_id]);
    wire_requests(&server, 1);
}

struct FailingTool(bool);
#[async_trait::async_trait]
impl KitHost for FailingTool {
    async fn call_tool(
        &self,
        _: &ToolCall,
        _: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        Err(ToolError {
            code: "host_failure".into(),
            message: "side effect uncertain".into(),
            unknown: self.0,
        })
    }
}
#[tokio::test]
async fn host_tool_errors_preserve_known_vs_unknown_side_effects() {
    let call = ToolCall {
        id: "call".into(),
        name: "write".into(),
        args: json!({}),
    };
    for unknown in [false, true] {
        let tools = crate::tools::HostTools::new(Arc::new(FailingTool(unknown)));
        let result = tools.execute(&call, &CancellationToken::new()).await;
        if unknown {
            assert!(
                matches!(result, ToolExecution::Unknown { reason } if reason == "side effect uncertain")
            );
        } else {
            assert!(
                matches!(result, ToolExecution::Error { code, payload: None, .. } if code == "host_failure")
            );
        }
    }
}

#[test]
fn snapshot_clone_keeps_public_error_without_diagnostic_cause() {
    use std::error::Error;
    let error = KitError::internal(std::io::Error::other("private snapshot diagnostic"))
        .with_reason("save_failed");
    let snapshot = crate::RunSnapshot {
        run_id: "run".into(),
        conversation_id: None,
        turn_id: None,
        status: crate::RunStatus::Failed,
        cancel_requested: false,
        error: Some(error),
        exhausted: None,
    };
    let copied = snapshot.clone();
    assert!(snapshot.error.as_ref().unwrap().source().is_some());
    assert!(copied.error.as_ref().unwrap().source().is_none());
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        serde_json::to_value(&copied).unwrap()
    );
    assert!(!format!("{copied:?}").contains("private snapshot diagnostic"));
}

#[derive(Default)]
struct BlockingTool {
    started: Notify,
    release: Notify,
    finished: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl KitHost for BlockingTool {
    fn tools(&self, _: &RunRequest, _: &str) -> Vec<HostTool> {
        vec![HostTool {
            spec: ToolSpec {
                name: "read_note".into(),
                description: "Wait for a dispatched host operation".into(),
                parameters: json!({"type":"object","properties":{}}),
                approval: Approval::Auto,
                effect: ToolEffect::Read,
                source: ToolSource::Host,
            },
        }]
    }
    async fn call_tool(
        &self,
        _: &ToolCall,
        _: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(ToolOutput {
            text: "finished".into(),
            payload: None,
        })
    }
    fn run_finished(&self, id: &str) {
        self.finished.lock().unwrap().push(id.into());
    }
}

#[tokio::test]
async fn concurrent_and_repeated_shutdown_wait_for_dispatched_host_work() {
    let server = upstream(vec![sse(TWO_TOOLS)]).await;
    let url = format!("{}/v1", server.url());
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &url, "wait");
    let host = Arc::new(BlockingTool::default());
    let service = service(host.clone(), providers, None);
    let owner = RunOwner::new();
    let (sink, mut receiver) = events();
    let accepted = service
        .start(context(&owner), request.clone(), sink)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), host.started.notified())
        .await
        .unwrap();

    let first = service.shutdown(Duration::from_millis(30));
    let second = service.shutdown(Duration::from_secs(2));
    tokio::pin!(first, second);
    std::future::poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        assert!(second.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    first.await;
    assert!(host.finished.lock().unwrap().is_empty());
    std::future::poll_fn(|cx| {
        assert!(second.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;

    let third = service.shutdown(Duration::from_secs(2));
    tokio::pin!(third);
    std::future::poll_fn(|cx| {
        assert!(third.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    host.release.notify_one();
    tokio::join!(second, third);
    assert_end(&until_end(&mut receiver).await, EndStatus::Cancelled);
    assert_eq!(*host.finished.lock().unwrap(), [accepted.run_id]);
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
    wire_requests(&server, 1);
}

/// Blocks inside preparation (host `tools`, on the blocking pool) until released.
struct PreparingHost {
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    finished: Mutex<Vec<String>>,
}
impl KitHost for PreparingHost {
    fn tools(&self, _: &RunRequest, _: &str) -> Vec<HostTool> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Vec::new()
    }
    fn run_finished(&self, id: &str) {
        self.finished.lock().unwrap().push(id.into());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_a_preparing_acceptance_across_an_expired_grace() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), "http://127.0.0.1:9/v1", "preparing");
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let host = Arc::new(PreparingHost {
        entered,
        release: Mutex::new(release_rx),
        finished: Mutex::default(),
    });
    let service = service(host.clone(), providers, None);
    let owner = RunOwner::new();
    let (sink, mut receiver) = events();
    let start = tokio::spawn({
        let service = service.clone();
        async move { service.start(context(&owner), request, sink).await }
    });
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();

    // A short grace expires while preparation is blocked; the acceptance is not abandoned.
    service.shutdown(Duration::from_millis(30)).await;
    assert!(host.finished.lock().unwrap().is_empty());

    let shutdown = tokio::spawn({
        let service = service.clone();
        async move { service.shutdown(Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !shutdown.is_finished(),
        "drain must wait for the preparing acceptance"
    );
    release.send(()).unwrap();
    // Rejected after preparation because the service is closing; the host is released once.
    assert_eq!(
        start.await.unwrap().unwrap_err().code(),
        KitErrorCode::Cancelled
    );
    shutdown.await.unwrap();
    assert_eq!(host.finished.lock().unwrap().len(), 1);
    assert!(receiver.try_recv().is_err());
}

/// Records the cutoff of every image purge, then delegates.
struct PurgeProbe {
    inner: Arc<SqliteStore>,
    cutoffs: std::sync::mpsc::Sender<i64>,
}
impl ConversationStore for PurgeProbe {
    fn list(
        &self,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::conversations::ConversationSummary>, KitError> {
        self.inner.list(scope, limit)
    }
    fn load(&self, id: &str) -> Result<crate::conversations::Conversation, KitError> {
        self.inner.load(id)
    }
    fn reserve_action(
        &self,
        action: &crate::conversations::ActionRequest,
        validate: &mut dyn FnMut(
            &crate::conversations::ReservationInput,
        ) -> Result<aether_chat::RunInput, KitError>,
    ) -> Result<crate::conversations::TurnReservation, KitError> {
        self.inner.reserve_action(action, validate)
    }
    fn finish_turn(&self, turn: TurnHandle, record: TurnRecord) -> Result<(), KitError> {
        self.inner.finish_turn(turn, record)
    }
    fn delete(&self, id: &str) -> Result<(), KitError> {
        self.inner.delete(id)
    }
    fn rename(
        &self,
        id: &str,
        title: &str,
    ) -> Result<crate::conversations::ConversationSummary, KitError> {
        self.inner.rename(id, title)
    }
    fn edit_turn(
        &self,
        c: &str,
        t: &str,
        text: &str,
        expected: &str,
    ) -> Result<crate::conversations::Conversation, KitError> {
        self.inner.edit_turn(c, t, text, expected)
    }
    fn remove_turn(
        &self,
        c: &str,
        t: &str,
    ) -> Result<crate::conversations::Conversation, KitError> {
        self.inner.remove_turn(c, t)
    }
    fn fork(&self, c: &str, t: &str) -> Result<crate::conversations::Conversation, KitError> {
        self.inner.fork(c, t)
    }
    fn select_turn(
        &self,
        c: &str,
        t: &str,
    ) -> Result<crate::conversations::Conversation, KitError> {
        self.inner.select_turn(c, t)
    }
    fn purge_images(&self, before_ms: i64) -> Result<usize, KitError> {
        let cleared = self.inner.purge_images(before_ms);
        self.cutoffs.send(before_ms).unwrap();
        cleared
    }
    fn add_generated(&self, turn_id: &str, images: &[ImagePart]) -> Result<(), KitError> {
        self.inner.add_generated(turn_id, images)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn image_ttl_maintenance_uses_the_injected_clock() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), "http://127.0.0.1:9/v1", "picture");
    let inner = Arc::new(SqliteStore::open(dir.path()).unwrap());
    request.images = vec![part("aW1n")];
    let turn = reserve(&inner, providers.clone(), request);
    let stored = || {
        inner.load(&turn.conversation_id).unwrap().turns[0].images[0]
            .data
            .clone()
    };
    let ttl = crate::conversations::IMAGE_TTL_DAYS * 86_400_000;
    let created = aether_support::clock::now_millis();
    // One minute before and after expiry, on a fixed clock: no waiting for real days.
    for (now, kept) in [
        (created + ttl - 60_000, true),
        (created + ttl + 60_000, false),
    ] {
        let (cutoffs, seen) = std::sync::mpsc::channel();
        let mut deps = RunDependencies::new(providers.clone());
        deps.conversations = Some(Arc::new(PurgeProbe {
            inner: inner.clone(),
            cutoffs,
        }));
        deps.clock = Arc::new(move || now);
        let service = RunService::new(
            Arc::new(Host),
            RunConfig::default(),
            deps,
            AddressPolicy::default(),
        )
        .unwrap();
        // The first tick fires immediately; wait for that purge to complete, not for time.
        let cutoff = tokio::task::spawn_blocking(move || seen.recv().unwrap())
            .await
            .unwrap();
        assert_eq!(cutoff, now - ttl);
        assert_eq!(stored().is_some(), kept, "now={now}");
        service.shutdown(Duration::from_secs(2)).await;
    }
}
