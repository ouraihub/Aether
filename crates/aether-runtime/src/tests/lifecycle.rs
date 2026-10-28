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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    // Keep both model requests pending; cancellation, not a fake engine, ends them.
    let server = tokio::spawn(async move {
        let mut sockets = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            sockets.push(socket);
        }
        std::future::pending::<()>().await;
    });
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
    server.abort();
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));
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
    server.await.unwrap();
}

#[tokio::test]
async fn save_failure_replaces_completed_end_and_snapshot_without_reexecution() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));
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
    assert_eq!(server.await.unwrap().len(), 1);
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![TWO_TOOLS]));
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
    assert_eq!(server.await.unwrap().len(), 1);
}
