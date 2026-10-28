use super::*;

#[derive(Default)]
struct EffectiveHost {
    seen: Mutex<Vec<(&'static str, RunRequest)>>,
    finished: Mutex<Vec<String>>,
}
impl EffectiveHost {
    fn observe(&self, method: &'static str, request: &RunRequest) {
        self.seen.lock().unwrap().push((method, request.clone()));
    }
}
impl KitHost for EffectiveHost {
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, KitError> {
        self.observe("model", request);
        crate::default_select(request)
    }
    fn system_prompt(&self, request: &RunRequest) -> Option<String> {
        self.observe("system", request);
        Some("safety rules".into())
    }
    fn context(&self, request: &RunRequest) -> Result<Option<crate::ContextBlock>, KitError> {
        self.observe("context", request);
        Ok(None)
    }
    fn tools(&self, request: &RunRequest, _: &str) -> Vec<HostTool> {
        self.observe("tools", request);
        Vec::new()
    }
    fn permission_modes(&self, request: &RunRequest) -> Vec<PermissionMode> {
        self.observe("permissions", request);
        PermissionMode::BASELINE.to_vec()
    }
    fn run_finished(&self, run_id: &str) {
        self.finished.lock().unwrap().push(run_id.into());
    }
}

#[tokio::test]
async fn effective_retry_source_images_reach_every_host_hook_and_real_upstream() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), &url, "source question");
    request.images = vec![part("aW4x")];
    let model = request.model.clone().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let source = reserve(&store, providers.clone(), request.clone());
    store
        .finish_turn(source.clone(), reply_record(&model, "source answer"))
        .unwrap();
    request.conversation_id = Some(source.conversation_id.clone());
    request.retry_turn_id = Some(source.turn_id.clone());
    request.input = "client cannot replace source".into();
    request.images.clear();
    request.model = None;
    let host = Arc::new(EffectiveHost::default());
    let service = service(host.clone(), providers, Some(store.clone()));
    let owner = RunOwner::new();
    let (sink, mut receiver) = events();
    let accepted = service.start(context(&owner), request, sink).await.unwrap();
    assert_end(&until_end(&mut receiver).await, EndStatus::Completed);
    service.shutdown(Duration::from_secs(2)).await;
    {
        let seen = host.seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        for (_, effective) in seen.iter() {
            assert_eq!(effective.input, "source question");
            assert_eq!(effective.images, [part("aW4x")]);
            assert_eq!(effective.model.as_ref(), Some(&model));
        }
    }
    assert_eq!(*host.finished.lock().unwrap(), [accepted.run_id]);
    let conversation = store.load(&source.conversation_id).unwrap();
    let retry = conversation.turns.last().unwrap();
    assert_eq!(retry.input, "source question");
    assert_eq!(
        retry.supersedes_id.as_deref(),
        Some(source.turn_id.as_str())
    );
    assert_eq!(retry.images[0].data.as_deref(), Some("aW4x"));
    let received = server.await.unwrap();
    assert!(received[0].contains("source question"));
    assert!(received[0].contains("data:image/png;base64,aW4x"));
    assert!(!received[0].contains("client cannot replace source"));
}

#[tokio::test]
async fn retry_regenerate_and_translation_keep_source_model_and_branch_ancestry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(upstream(listener, vec![HELLO]));
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), &url, "q1");
    let model = request.model.clone().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let first = reserve(&store, providers.clone(), request.clone());
    store
        .finish_turn(first.clone(), reply_record(&model, "original answer"))
        .unwrap();
    request.conversation_id = Some(first.conversation_id.clone());
    request.retry_turn_id = Some(first.turn_id.clone());
    let alternative = reserve(&store, providers.clone(), request.clone());
    store
        .finish_turn(
            alternative.clone(),
            reply_record(&model, "alternative answer"),
        )
        .unwrap();
    request.retry_turn_id = None;
    request.input = "q2".into();
    let second = reserve(&store, providers.clone(), request.clone());
    store
        .finish_turn(second.clone(), reply_record(&model, "second answer"))
        .unwrap();
    request.input = "q3".into();
    let normal = prepared(
        Arc::new(Host),
        providers.clone(),
        Some(store.clone()),
        request.clone(),
    )
    .unwrap();
    assert!(normal.input.history.contains(&Message::User("q2".into())));
    assert!(
        !serde_json::to_string(&normal.input.history)
            .unwrap()
            .contains("alternative answer")
    );
    request.model = None;
    request.retry_turn_id = Some(second.turn_id.clone());
    let retry = prepared(
        Arc::new(EffectiveHost::default()),
        providers.clone(),
        Some(store.clone()),
        request.clone(),
    )
    .unwrap();
    assert_eq!(retry.input.input, "q2");
    assert_eq!(retry.input.history.len(), 2);
    assert!(retry.input.history.contains(&Message::User("q1".into())));
    assert!(
        matches!(&retry.input.system[0], aether_chat::SystemPart::Host(text) if text == "safety rules")
    );
    assert_eq!(
        retry.input.connection.endpoint_id.as_deref(),
        Some(model.endpoint_id.as_str())
    );
    request.retry_turn_id = None;
    request.regenerate = true;
    let regenerate = prepared(
        Arc::new(Host),
        providers.clone(),
        Some(store.clone()),
        request.clone(),
    )
    .unwrap();
    assert_eq!(regenerate.input.input, "q2");
    assert_eq!(regenerate.input.history, retry.input.history);
    request.regenerate = false;
    request.translate_turn_id = Some(alternative.turn_id.clone());
    request.target_language = Some("French".into());
    let translation = prepared(
        Arc::new(EffectiveHost::default()),
        providers.clone(),
        Some(store.clone()),
        request.clone(),
    )
    .unwrap();
    assert_eq!(
        translation.input.input,
        "Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\nalternative answer"
    );
    assert_eq!(
        translation.input.connection.endpoint_id.as_deref(),
        Some(model.endpoint_id.as_str())
    );
    assert_eq!(translation.input.history.len(), 2);
    assert!(
        !serde_json::to_string(&translation.input.history)
            .unwrap()
            .contains("second answer")
    );
    let service = service(
        Arc::new(EffectiveHost::default()),
        providers.clone(),
        Some(store.clone()),
    );
    let (sink, mut receiver) = events();
    service
        .start(context(&RunOwner::new()), request.clone(), sink)
        .await
        .unwrap();
    assert_end(&until_end(&mut receiver).await, EndStatus::Completed);
    service.shutdown(Duration::from_secs(2)).await;
    let received = server.await.unwrap();
    assert!(received[0].contains("Translate the following reply into French"));
    assert!(received[0].contains("safety rules"));
    request.translate_turn_id = Some("foreign".into());
    assert_eq!(
        prepared(
            Arc::new(Host),
            providers.clone(),
            Some(store.clone()),
            request.clone()
        )
        .err()
        .unwrap()
        .code(),
        KitErrorCode::NotFound
    );
    request.translate_turn_id = None;
    request.target_language = None;
    request.retry_turn_id = Some(first.turn_id);
    let mut running_request = request.clone();
    running_request.retry_turn_id = None;
    running_request.model = Some(model.clone());
    running_request.input = "running".into();
    let running = reserve(&store, providers.clone(), running_request);
    let error = prepared(
        Arc::new(Host),
        providers.clone(),
        Some(store.clone()),
        request.clone(),
    )
    .err()
    .unwrap();
    assert_eq!(error.details().reason(), Some("turn_running"));
    store
        .finish_turn(running, reply_record(&model, "done"))
        .unwrap();
    providers.delete(&providers.list().unwrap()[0].id).unwrap();
    let error = prepared(
        Arc::new(Host),
        providers.clone(),
        Some(store.clone()),
        request.clone(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code(), KitErrorCode::NeedsConfig);
    assert_eq!(error.details().reason(), Some("source_model_unavailable"));
    request.model = Some(ModelRef {
        endpoint_id: "missing".into(),
        model_id: "missing".into(),
    });
    assert_eq!(
        prepared(Arc::new(Host), providers, Some(store), request)
            .err()
            .unwrap()
            .details()
            .reason(),
        Some("source_model_unavailable")
    );
}

#[test]
fn painting_choices_keep_earlier_generated_images_and_validate_ids_and_expiry() {
    use crate::tools::painting::{ImageChoice, Painting, image_choices, resolve_images};
    let dir = tempfile::tempdir().unwrap();
    let (providers, mut request) = local_model(dir.path(), "http://127.0.0.1:9/v1", "draw");
    let model = request.model.clone().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    request.images = vec![part("aW4x")];
    let first = reserve(&store, providers.clone(), request.clone());
    store
        .add_generated(&first.turn_id, &[part("Z2VuMQ==")])
        .unwrap();
    store
        .finish_turn(first.clone(), reply_record(&model, "done"))
        .unwrap();
    request.conversation_id = Some(first.conversation_id.clone());
    request.input = "q2".into();
    request.images = vec![part("bm93")];
    let preparation = prepared(Arc::new(Host), providers, Some(store.clone()), request).unwrap();
    let source = store.load(&first.conversation_id).unwrap();
    let path = crate::conversations::history_path(&source.turns, Some(&first.turn_id)).unwrap();
    let choices = image_choices(&preparation.input.images, &path);
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
    let history: Option<(Arc<dyn ConversationStore>, String)> =
        Some((store.clone(), first.conversation_id));
    let mut connection = preparation.input.connection;
    connection.can_vision = false;
    let mut painting = Painting::new(
        connection,
        AddressPolicy::default(),
        None,
        choices.clone().into(),
        history.clone(),
    );
    assert!(
        painting.spec().parameters["properties"]
            .get("image_ids")
            .is_none()
    );
    assert_eq!(
        resolve_images(
            &choices,
            history.as_ref(),
            false,
            &json!({"image_ids":["image-2"]})
        )
        .unwrap_err()
        .details()
        .reason(),
        Some("unsupported")
    );
    painting.connection.can_vision = true;
    let spec = painting.spec();
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
        resolve_images(
            &choices,
            history.as_ref(),
            true,
            &json!({"image_ids":["image-2","image-1"]})
        )
        .unwrap(),
        [part("Z2VuMQ=="), part("bm93")]
    );
    assert!(
        resolve_images(&choices, history.as_ref(), true, &json!({"prompt":"x"}))
            .unwrap()
            .is_empty()
    );
    for (args, reason) in [
        (json!({"image_ids":["image-9"]}), "unknown"),
        (json!({"image_ids":["x"]}), "unknown"),
        (json!({"image_ids":vec!["image-1"; 5]}), "too_many"),
    ] {
        assert_eq!(
            resolve_images(&choices, history.as_ref(), true, &args)
                .unwrap_err()
                .details()
                .reason(),
            Some(reason)
        );
    }
    store.purge_images(i64::MAX).unwrap();
    assert_eq!(
        resolve_images(
            &choices,
            history.as_ref(),
            true,
            &json!({"image_ids":["image-2"]})
        )
        .unwrap_err()
        .details()
        .reason(),
        Some("expired")
    );
}

#[test]
fn conversation_projection_preserves_public_usage_branch_and_reasoning_without_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), "http://127.0.0.1:9/v1", "question");
    let model = request.model.clone().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let turn = reserve(&store, providers, request);
    let mut record = reply_record(&model, "answer");
    let block = |protocol, value| aether_support::ReasoningBlock {
        protocol,
        endpoint_id: Some("e1".into()),
        model_id: Some("m1".into()),
        value,
    };
    if let Message::Assistant { reasoning, .. } = &mut record.messages[0] {
        *reasoning = vec![
            block(
                aether_support::Protocol::AnthropicMessages,
                json!({"type":"thinking","thinking":"visible thought","signature":"private signature"}),
            ),
            block(
                aether_support::Protocol::AnthropicMessages,
                json!({"type":"redacted_thinking","data":"private data"}),
            ),
            block(
                aether_support::Protocol::OpenaiResponses,
                json!({"type":"reasoning","summary":[{"type":"summary_text","text":"visible summary"}],"encrypted_content":"private encrypted"}),
            ),
            block(aether_support::Protocol::OpenaiChat, json!("visible chat")),
        ];
    }
    record.cost = Some(aether_support::Cost {
        amount: 0.125,
        currency: aether_support::Currency::Usd,
    });
    store.finish_turn(turn.clone(), record).unwrap();
    let wire = serde_json::to_value(crate::contract::conversation_dto::conversation(
        store.load(&turn.conversation_id).unwrap(),
    ))
    .unwrap();
    assert_eq!(wire["summary"]["selectedTurnId"], turn.turn_id);
    let saved = &wire["turns"][0];
    assert_eq!(saved["questionId"], turn.turn_id);
    assert!(saved["parentId"].is_null());
    assert!(saved["supersedesId"].is_null());
    assert_eq!(saved["kind"], "reply");
    assert_eq!(saved["endpointId"], model.endpoint_id);
    assert_eq!(saved["cost"], json!({"amount":0.125,"currency":"USD"}));
    assert_eq!(saved["usage"]["inputTokens"], 10);
    assert_eq!(
        saved["reasoningText"],
        "visible thoughtvisible summaryvisible chat"
    );
    assert!(!wire.to_string().contains("private"));
    let accepted = serde_json::to_value(RunAccepted {
        run_id: "run".into(),
        conversation_id: Some(turn.conversation_id),
        turn_id: Some(turn.turn_id.clone()),
    })
    .unwrap();
    assert_eq!(accepted["turnId"], turn.turn_id);
}
