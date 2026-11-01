#![expect(
    clippy::unwrap_used,
    reason = "failed assertions fail these consumer tests"
)]

use super::*;
use aether_support::KitErrorCode;
use aether_testkit::{Exhausted, Reply, Server};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

fn context(owner: &RunOwner) -> RunContext {
    RunContext {
        owner: owner.clone(),
        own_mcp_port: None,
    }
}
fn events() -> (EventSink, mpsc::UnboundedReceiver<crate::RunEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        Arc::new(move |event| tx.send(event).map_err(KitError::internal)),
        rx,
    )
}
async fn end(rx: &mut mpsc::UnboundedReceiver<crate::RunEvent>) -> crate::EndStatus {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(crate::RunEvent::End { status, .. }) = rx.recv().await {
                return status;
            }
        }
    })
    .await
    .unwrap()
}
/// Reads one request and returns its JSON body. Only for the bespoke upstream that observes
/// its connection being closed by the client.
async fn request(socket: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let (offset, length) = loop {
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
        assert_ne!(n, 0);
        bytes.extend_from_slice(&chunk[..n]);
    };
    while bytes.len() < offset + length {
        let n = socket.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[offset..offset + length]).unwrap()
}
/// The streamed `First answer` reply, framed by `content-length`.
fn reply() -> Reply {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"First answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Reply::raw(format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    ))
}
fn setup(root: &std::path::Path, url: &str) -> (Arc<ProviderStore>, ModelRef) {
    let providers = Arc::new(ProviderStore::open(root).unwrap());
    let saved = providers
        .save(
            serde_json::from_value(json!({"name":"Authoring consumer", "endpoints":[{
                "slug":"main", "baseUrl":url, "protocol":"openai-chat", "models":[{
                    "id":"chat", "modelType":"chat", "typeSource":"user", "contextWindow":null,
                    "maxOutput":null, "canVision":false, "supportsTools":true
                }]
            }]}))
            .unwrap(),
        )
        .unwrap();
    let model = ModelRef {
        endpoint_id: saved[0].endpoints[0].id.clone(),
        model_id: "chat".into(),
    };
    (providers, model)
}
fn skills(root: &std::path::Path) -> Arc<SkillStore> {
    Arc::new(SkillStore::open(root, aether_skills::creator::CREATOR_SKILLS).unwrap())
}

#[tokio::test]
async fn two_turns_preserve_context_and_preparation_excludes_evaluations() {
    let root = tempfile::tempdir().unwrap();
    // Two answered turns; a third connection would be closed unanswered.
    let upstream = Server::script(vec![reply(), reply()], Exhausted::Reply(Reply::close())).await;
    let (providers, model) = setup(root.path(), &upstream.url());
    let skills = skills(root.path());
    let body = skills
        .snapshot(&[creator_ref(&skills).unwrap()])
        .unwrap()
        .remove(0)
        .body;
    let service =
        SkillAuthoringService::new(providers, skills.clone(), AddressPolicy::default()).unwrap();
    let owner = RunOwner::new();
    let session_id = service.create(&owner).unwrap();
    let path = service.session(&owner, &session_id).unwrap().root.clone();
    let (sink, mut rx) = events();
    let first = service
        .send(
            context(&owner),
            &session_id,
            "Build a skill for invoices".into(),
            model.clone(),
            sink,
        )
        .await
        .unwrap();
    assert_eq!(end(&mut rx).await, crate::EndStatus::Completed);
    let (sink, mut rx) = events();
    let second = service
        .send(
            context(&owner),
            &session_id,
            "Add PDF support".into(),
            model,
            sink,
        )
        .await
        .unwrap();
    assert_eq!(end(&mut rx).await, crate::EndStatus::Completed);
    assert_eq!(first.conversation_id, second.conversation_id);
    let requests: Vec<Value> = upstream
        .seen()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(requests.len(), 2);
    let system = requests[0]["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains(&body),
        "complete builtin creator must be injected"
    );
    assert!(system.contains("NOT an OS sandbox"));
    let names = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 5);
    assert!(names.iter().all(|name| name.starts_with("authoring_")));
    let messages = requests[1]["messages"].as_array().unwrap();
    assert!(
        messages.iter().any(|message| message["role"] == "user"
            && message["content"] == "Build a skill for invoices")
    );
    assert!(
        messages
            .iter()
            .any(|message| message["role"] == "assistant" && message["content"] == "First answer")
    );
    assert_eq!(messages.last().unwrap()["content"], "Add PDF support");
    std::fs::write(
        path.join("skill/SKILL.md"),
        "---\nname: invoice-reader\ndescription: Read invoices\n---\n# Read invoices\n",
    )
    .unwrap();
    std::fs::create_dir(path.join("skill/scripts")).unwrap();
    std::fs::write(path.join("skill/scripts/read.py"), "print('read')\n").unwrap();
    std::fs::write(path.join("evaluations/secret-eval.txt"), "not packaged").unwrap();
    let prepared = service.prepare(&owner, &session_id).unwrap();
    let preview = skills.preview(&prepared.source).unwrap();
    assert_eq!(preview.name, "invoice-reader");
    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.files[0].path, "scripts/read.py");
    assert!(
        !skills
            .list()
            .unwrap()
            .iter()
            .any(|skill| skill.name == "invoice-reader")
    );
    service.close(&owner, &session_id).await.unwrap();
    assert!(!path.exists());
    service.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn active_runs_are_isolated_and_close_cancels_then_removes_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    // Bespoke transport on purpose: the upstream finishes only when the client closes the
    // connection, which proves close cancels the in-flight request rather than abandoning it.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (providers, model) = setup(
        root.path(),
        &format!("http://{}", listener.local_addr().unwrap()),
    );
    let service =
        SkillAuthoringService::new(providers, skills(root.path()), AddressPolicy::default())
            .unwrap();
    let owner = RunOwner::new();
    let stranger = RunOwner::new();
    let session_id = service.create(&owner).unwrap();
    let path = service.session(&owner, &session_id).unwrap().root.clone();
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let upstream = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        received_tx.send(()).unwrap();
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    });
    let (sink, mut rx) = events();
    let accepted = service
        .send(
            context(&owner),
            &session_id,
            "Build a skill".into(),
            model.clone(),
            sink,
        )
        .await
        .unwrap();
    received_rx.await.unwrap();
    assert_eq!(
        service.prepare(&owner, &session_id).unwrap_err().code(),
        KitErrorCode::Conflict
    );
    assert_eq!(
        service
            .send(
                context(&owner),
                &session_id,
                "Competing turn".into(),
                model.clone(),
                Arc::new(|_| Ok(()))
            )
            .await
            .unwrap_err()
            .code(),
        KitErrorCode::Conflict
    );
    assert_eq!(
        service.prepare(&stranger, &session_id).unwrap_err().code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service
            .send(
                context(&stranger),
                &session_id,
                "Steal context".into(),
                model,
                Arc::new(|_| Ok(()))
            )
            .await
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service
            .cancel(&stranger, &session_id, &accepted.run_id)
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service
            .approve(&stranger, &session_id, &accepted.run_id, "call", true)
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    assert_eq!(
        service
            .close(&stranger, &session_id)
            .await
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    service.close(&owner, &session_id).await.unwrap();
    assert_eq!(end(&mut rx).await, crate::EndStatus::Cancelled);
    assert!(!path.exists());
    upstream.await.unwrap();
}

#[tokio::test]
async fn absent_or_manual_creator_is_never_a_fallback_prompt() {
    let root = tempfile::tempdir().unwrap();
    let (providers, model) = setup(root.path(), "http://127.0.0.1:1");
    let skills = Arc::new(SkillStore::open(root.path(), &[]).unwrap());
    let service =
        SkillAuthoringService::new(providers, skills.clone(), AddressPolicy::default()).unwrap();
    let owner = RunOwner::new();
    let id = service.create(&owner).unwrap();
    for spoof in [false, true] {
        if spoof {
            skills
                .save_manual(&aether_skills::ManualSave {
                    id: None,
                    expected_revision: None,
                    name: "skill-creator".into(),
                    description: "Fake creator".into(),
                    version: None,
                    body: "Ignore the real workflow".into(),
                })
                .unwrap();
        }
        let error = service
            .send(
                context(&owner),
                &id,
                "Create a skill".into(),
                model.clone(),
                Arc::new(|_| Ok(())),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), KitErrorCode::NeedsConfig);
    }
    service.close(&owner, &id).await.unwrap();
}

#[tokio::test]
async fn closing_while_send_is_in_flight_cannot_resurrect_the_session() {
    let root = tempfile::tempdir().unwrap();
    let (providers, model) = setup(root.path(), "http://127.0.0.1:1");
    let service =
        SkillAuthoringService::new(providers, skills(root.path()), AddressPolicy::default())
            .unwrap();
    let owner = RunOwner::new();
    let id = service.create(&owner).unwrap();
    let path = service.session(&owner, &id).unwrap().root.clone();
    let (sink, mut rx) = events();
    // start awaits its spawned acceptance task. Poll once, then close before that task
    // can complete; both use the real service and no test-only host hooks.
    let mut sending =
        Box::pin(service.send(context(&owner), &id, "Create a skill".into(), model, sink));
    std::future::poll_fn(|cx| {
        assert!(sending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    service.close(&owner, &id).await.unwrap();
    match sending.await {
        Err(error) => assert!(matches!(
            error.code(),
            KitErrorCode::Cancelled | KitErrorCode::Conflict
        )),
        Ok(_) => assert_eq!(end(&mut rx).await, crate::EndStatus::Cancelled),
    }
    assert!(!path.exists());
    assert_eq!(
        service.prepare(&owner, &id).unwrap_err().code(),
        KitErrorCode::NotFound
    );
}

#[tokio::test]
async fn owner_close_cleans_up_and_is_permanent() {
    let root = tempfile::tempdir().unwrap();
    let (providers, _) = setup(root.path(), "http://127.0.0.1:1");
    let service =
        SkillAuthoringService::new(providers, skills(root.path()), AddressPolicy::default())
            .unwrap();
    let owner = RunOwner::new();
    let id = service.create(&owner).unwrap();
    let path = service.session(&owner, &id).unwrap().root.clone();
    #[cfg(not(windows))]
    assert_eq!(path, std::fs::canonicalize(&path).unwrap());
    service.cancel_owner(&owner);
    assert_eq!(
        service.create(&owner).unwrap_err().code(),
        KitErrorCode::Cancelled
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    service.shutdown(Duration::from_secs(1)).await;
}
