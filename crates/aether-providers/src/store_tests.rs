
#![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
use aether_support::KitErrorCode;
use serde_json::{Value, json};
use super::*;
const KEY: &str = "sk-test-0123456789abcd";
    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn request(value: Value) -> ProviderSave {
        serde_json::from_value(value).unwrap()
    }

    fn model(id: &str) -> Value {
        json!({ "id": id, "modelType": "chat", "typeSource": "inferred", "contextWindow": 200_000,
                "maxOutput": null, "canVision": true, "supportsTools": true })
    }

    fn endpoint(id: Option<&str>, slug: &str, models: &[&str]) -> Value {
        let mut value = json!({ "slug": slug, "baseUrl": format!("https://{slug}.example.com/"),
            "protocol": "openai-chat", "models": models.iter().map(|m| model(m)).collect::<Vec<_>>() });
        if let Some(id) = id {
            value["id"] = json!(id);
        }
        value
    }

    /// Keys of an endpoint in this request's try order.
    fn keys_of(store: &Store, endpoint_id: &str) -> Vec<String> {
        let connection = store.endpoint_connection(endpoint_id).unwrap();
        connection
            .api_keys
            .iter()
            .map(|key| key.as_str().to_owned())
            .collect()
    }

    fn save_key(store: &Store, value: Value) -> Result<Vec<Provider>, KitError> {
        store.save_key(serde_json::from_value(value).unwrap())
    }

    /// A saved provider with one endpoint `a` holding model `m1`; returns (provider id, endpoint id).
    fn saved(store: &Store) -> (String, String) {
        let saved = store
            .save(request(
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &["m1"])] }),
            ))
            .unwrap();
        (saved[0].id.clone(), saved[0].endpoints[0].id.clone())
    }

    fn reason(error: &KitError) -> (KitErrorCode, Option<&str>, Option<&str>) {
        (
            error.code(),
            error.details().field(),
            error.details().reason(),
        )
    }

    #[test]
    fn saved_null_reasoning_is_inferred_consistently_without_reconfiguration() {
        let (dir, store) = store();
        let mut ep = endpoint(
            None,
            "auto",
            &["claude-opus-5-5", "claude-sonnet-5-5", "unknown"],
        );
        ep["protocol"] = json!("anthropic-messages");
        let saved = store
            .save(request(json!({"name":"Automatic","endpoints":[ep]})))
            .unwrap();
        let id = &saved[0].endpoints[0].id;
        let reference = ModelRef {
            endpoint_id: id.clone(),
            model_id: "claude-sonnet-5-5".into(),
        };
        let expected = infer_reasoning(
            &reference.model_id,
            Protocol::AnthropicMessages,
            "https://api.anthropic.com/v1",
            None,
        )
        .unwrap();
        assert_eq!(
            saved[0].endpoints[0].models[1].reasoning.as_ref(),
            Some(&expected)
        );
        assert_eq!(
            store.resolve(&reference).unwrap().reasoning,
            Some(expected.clone())
        );
        let options = store.model_options(ModelFilter::default()).unwrap();
        assert!(
            !options[0]
                .reasoning
                .as_ref()
                .unwrap()
                .efforts
                .contains(&aether_support::ReasoningEffort::None)
        );
        assert_eq!(options[1].reasoning.as_ref(), Some(&expected));
        assert!(options[2].reasoning.is_none());
        assert!(
            store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT reasoning FROM models WHERE model_id='claude-sonnet-5-5'",
                    [],
                    |row| row.get::<_, Option<String>>(0)
                )
                .unwrap()
                .is_none()
        );
        let override_json = r#"{"dialect":"anthropic-adaptive","efforts":["high"]}"#;
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE models SET reasoning=?1 WHERE model_id='claude-sonnet-5-5'",
                [override_json],
            )
            .unwrap();
        drop(store);
        let restarted = Store::open(dir.path()).unwrap();
        assert_eq!(
            restarted
                .resolve(&reference)
                .unwrap()
                .reasoning
                .unwrap()
                .efforts,
            vec![aether_support::ReasoningEffort::High]
        );
    }

    #[test]
    fn version_two_upgrade_preserves_models_and_native_declarations() {
        for already_has_native in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("providers.db");
            let db = Db::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, key_ciphertext BLOB, position INTEGER NOT NULL, UNIQUE(provider_id, slug));
                 CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, PRIMARY KEY(endpoint_id, model_id));
                 INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1);
                 INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-responses', X'01020304', 0);
                 INSERT INTO models VALUES ('e', 'saved-model', 'chat', 'user', 200000, 8192, 1, 1, 0, '{\"dialect\":\"openai\",\"efforts\":[\"low\",\"high\"]}');
                 PRAGMA user_version = 2;",
            ).unwrap();
            if already_has_native {
                db.execute_batch("ALTER TABLE models ADD COLUMN native_web_search INTEGER NOT NULL DEFAULT 0; UPDATE models SET native_web_search = 1;").unwrap();
            }
            drop(db);
            for _ in 0..2 {
                let store = Store::open(dir.path()).unwrap();
                let options = store.model_options(ModelFilter::default()).unwrap();
                assert_eq!(options[0].model_id, "saved-model");
                assert_eq!(options[0].native_web_search, already_has_native);
                assert_eq!(options[0].reasoning.as_ref().unwrap().efforts.len(), 2);
                assert_eq!(
                    store.list().unwrap()[0].endpoints[0].models[0].context_window,
                    Some(200_000)
                );
                let db = store.lock().unwrap();
                assert_eq!(
                    db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                        .unwrap(),
                    10
                );
            }
            assert!(path.with_file_name("providers.db.bak-v2").is_file());
        }
    }

    #[test]
    fn version_four_db_opens_with_old_keys_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("providers.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, key_ciphertext BLOB, position INTEGER NOT NULL, UNIQUE(provider_id, slug));
             CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, native_web_search INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint_id, model_id));
             INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1, 1);
             INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-chat', X'01020304', 0);
             INSERT INTO models VALUES ('e', 'm', 'chat', 'user', NULL, NULL, 0, 1, 0, NULL, 0);
             PRAGMA user_version = 4;",
        )
        .unwrap();
        drop(db);
        let store = Store::open(dir.path()).unwrap();
        let endpoint = &store.list().unwrap()[0].endpoints[0];
        assert_eq!((endpoint.id.as_str(), endpoint.models.len()), ("e", 1));
        assert!(endpoint.keys.is_empty());
        assert!(keys_of(&store, "e").is_empty());
    }

    #[test]
    fn disabled_provider_leaves_pickers_and_cannot_resolve_until_re_enabled() {
        let (_dir, store) = store();
        let saved = store
            .save(request(
                json!({"name":"Relay","endpoints":[endpoint(None, "a", &["m1"])]}),
            ))
            .unwrap();
        let (id, endpoint_id) = (saved[0].id.clone(), saved[0].endpoints[0].id.clone());
        assert!(saved[0].enabled);
        let reference = ModelRef {
            endpoint_id: endpoint_id.clone(),
            model_id: "m1".to_owned(),
        };
        let off = json!({"id": id, "name":"Relay","enabled":false,"endpoints":[endpoint(Some(&endpoint_id), "a", &["m1"])]});
        assert!(!store.save(request(off)).unwrap()[0].enabled);
        assert!(
            store
                .model_options(ModelFilter::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.resolve(&reference).unwrap_err().code(),
            KitError::not_found("model").code()
        );
        // Omitting `enabled` keeps it off; only an explicit `true` turns it back on.
        let keep = json!({"id": id, "name":"Relay 2","endpoints":[endpoint(Some(&endpoint_id), "a", &["m1"])]});
        assert!(!store.save(request(keep)).unwrap()[0].enabled);
        let on = json!({"id": id, "name":"Relay","enabled":true,"endpoints":[endpoint(Some(&endpoint_id), "a", &["m1"])]});
        assert!(store.save(request(on)).unwrap()[0].enabled);
        assert_eq!(
            store.model_options(ModelFilter::default()).unwrap().len(),
            1
        );
        assert!(store.resolve(&reference).is_ok());
    }

    #[test]
    fn native_search_declaration_survives_reopen_and_rejects_chat_protocol() {
        let (dir, store) = store();
        let mut e = endpoint(None, "a", &["m1"]);
        e["models"][0]["nativeWebSearch"] = json!(true);
        assert!(
            store
                .save(request(json!({"name":"Native","endpoints":[e.clone()]})))
                .is_err()
        );
        e["protocol"] = json!("openai-responses");
        let saved = store
            .save(request(json!({"name":"Native","endpoints":[e]})))
            .unwrap();
        let reference = ModelRef {
            endpoint_id: saved[0].endpoints[0].id.clone(),
            model_id: "m1".into(),
        };
        drop(store);
        let reopened = Store::open(dir.path()).unwrap();
        assert!(reopened.resolve(&reference).unwrap().native_web_search);
        assert!(reopened.model_options(ModelFilter::default()).unwrap()[0].native_web_search);
    }
    #[test]
    fn save_replaces_the_whole_provider_and_survives_reopen() {
        let (dir, store) = store();
        let saved = store
            .save(request(json!({ "name": "Acme", "endpoints": [
                endpoint(None, "a", &["m1", "m2"]), endpoint(None, "b", &["m3"]) ] })))
            .unwrap();
        let provider = &saved[0];
        let (a, b) = (&provider.endpoints[0], &provider.endpoints[1]);
        assert_eq!(a.probe_url, "https://a.example.com/v1/models");
        assert_eq!(a.models[0].context_window, Some(200_000));

        // Keep `a` (renamed to b's slug, models trimmed and reordered), drop `b`, add `c`.
        let saved = store
            .save(request(
                json!({ "id": provider.id, "name": "Acme 2", "endpoints": [
                endpoint(None, "a", &["n1"]), endpoint(Some(&a.id), "b", &["m2", "x"]) ] }),
            ))
            .unwrap();
        let endpoints = &saved[0].endpoints;
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Acme 2");
        assert_eq!(endpoints[1].id, a.id, "id carried back keeps the endpoint");
        assert_eq!(endpoints[1].slug, "b");
        let ids: Vec<_> = endpoints[1].models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m2", "x"]);
        assert_ne!(
            endpoints[0].id, b.id,
            "slug match alone does not keep an endpoint"
        );
        let gone = ModelRef {
            endpoint_id: b.id.clone(),
            model_id: "m3".to_owned(),
        };
        assert_eq!(
            store.resolve(&gone).err().unwrap().details().entity(),
            Some("model")
        );

        // Unknown endpoint id: rejected, nothing written.
        let error = store
            .save(request(json!({ "id": provider.id, "name": "X",
                "endpoints": [endpoint(Some(&b.id), "z", &[])] })))
            .err()
            .unwrap();
        assert_eq!(error.code(), KitErrorCode::NotFound);
        drop(store);
        assert_eq!(Store::open(dir.path()).unwrap().list().unwrap(), saved);
    }
    #[test]
    fn set_enabled_changes_only_provider_switch() {
        let (_dir, store) = store();
        let saved = store
            .save(request(
                json!({ "name": "Acme", "endpoints": [endpoint(None, "main", &["m1"])] }),
            ))
            .unwrap();
        let provider = &saved[0];
        let endpoint_id = provider.endpoints[0].id.clone();
        let changed = store.set_enabled(&provider.id, false).unwrap();
        assert!(!changed[0].enabled);
        assert_eq!(changed[0].name, "Acme");
        assert_eq!(changed[0].endpoints[0].id, endpoint_id);
        assert_eq!(changed[0].endpoints[0].models[0].id, "m1");
    }

    #[test]
    fn keys_are_added_patched_revealed_and_removed() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        let list = save_key(
            &store,
            json!({ "endpointId": ep, "key": format!(" {KEY} ") }),
        )
        .unwrap();
        let key = list[0].endpoints[0].keys[0].clone();
        assert_eq!(
            (
                key.label.as_str(),
                key.enabled,
                key.readable,
                key.masked.as_deref()
            ),
            ("", true, true, Some("sk-t**************abcd"))
        );
        let reference = ModelRef {
            endpoint_id: ep.clone(),
            model_id: "m1".to_owned(),
        };
        let resolved = store.resolve(&reference).unwrap();
        assert_eq!(resolved.api_keys.len(), 1);
        assert_eq!(resolved.api_keys[0].as_str(), KEY);
        assert_eq!(store.reveal_key(&key.id).unwrap().as_str(), KEY);

        // Label-only patch keeps the ciphertext byte for byte.
        let cipher = |store: &Store| -> Vec<u8> {
            store
                .lock()
                .unwrap()
                .query_row("SELECT key_ciphertext FROM endpoint_keys", [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        let before = cipher(&store);
        let list = save_key(
            &store,
            json!({ "endpointId": ep, "id": key.id, "label": " main " }),
        )
        .unwrap();
        assert_eq!(list[0].endpoints[0].keys[0].label, "main");
        assert_eq!(cipher(&store), before);
        save_key(
            &store,
            json!({ "endpointId": ep, "id": key.id, "key": "sk-other-key-000000" }),
        )
        .unwrap();
        assert_eq!(keys_of(&store, &ep), ["sk-other-key-000000"]);

        assert!(
            store.remove_key(&ep, &key.id).unwrap()[0].endpoints[0]
                .keys
                .is_empty()
        );
        assert_eq!(
            reason(&store.remove_key(&ep, &key.id).unwrap_err()).0,
            KitErrorCode::NotFound
        );
        assert_eq!(
            reason(&store.reveal_key(&key.id).unwrap_err()).0,
            KitErrorCode::NotFound
        );
    }

    #[test]
    fn rotation_skips_disabled_keys_and_all_disabled_is_no_key() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        for key in ["k1", "k2", "k3"] {
            save_key(&store, json!({ "endpointId": ep, "key": key })).unwrap();
        }
        let ids: Vec<String> = store.list().unwrap()[0].endpoints[0]
            .keys
            .iter()
            .map(|k| k.id.clone())
            .collect();
        save_key(
            &store,
            json!({ "endpointId": ep, "id": ids[1], "enabled": false }),
        )
        .unwrap();
        let reference = ModelRef {
            endpoint_id: ep.clone(),
            model_id: "m1".to_owned(),
        };
        let firsts: Vec<String> = (0..4)
            .map(|_| {
                store.resolve(&reference).unwrap().api_keys[0]
                    .as_str()
                    .to_owned()
            })
            .collect();
        assert_eq!(firsts, ["k1", "k3", "k1", "k3"]);
        // Each request still carries every enabled key for failover, starting at the cursor.
        assert_eq!(keys_of(&store, &ep), ["k1", "k3"]);
        assert_eq!(keys_of(&store, &ep), ["k3", "k1"]);

        for id in [&ids[0], &ids[2]] {
            save_key(
                &store,
                json!({ "endpointId": ep, "id": id, "enabled": false }),
            )
            .unwrap();
        }
        assert!(store.resolve(&reference).unwrap().api_keys.is_empty());
    }

    #[test]
    fn explicit_snapshots_reserve_once_and_survive_configuration_deletion() {
        let (_dir, store) = store();
        let (provider, endpoint_id) = saved(&store);
        for key in ["first", "second"] {
            save_key(&store, json!({ "endpointId": endpoint_id, "key": key })).unwrap();
        }
        let reference = ModelRef {
            endpoint_id: endpoint_id.clone(),
            model_id: "m1".into(),
        };
        let snapshot = store.snapshot(&reference).unwrap();
        assert_eq!(snapshot.endpoint.api_keys[0].as_str(), "first");
        assert_eq!(snapshot.model.as_ref().unwrap().id, "m1");
        let next = store.endpoint_snapshot(&endpoint_id).unwrap();
        assert_eq!(next.api_keys[0].as_str(), "second");
        let prepared = connection::prepare(snapshot).unwrap();
        assert_eq!(prepared.api_keys[0].as_str(), "first");
        assert_eq!(
            store.endpoint_snapshot(&endpoint_id).unwrap().api_keys[0].as_str(),
            "first"
        );
        store.delete(&provider).unwrap();
        let prepared = connection::prepare(ConnectionSnapshot {
            endpoint: next,
            model: None,
        })
        .unwrap();
        assert_eq!(prepared.api_keys[0].as_str(), "second");
        assert_eq!(
            store.snapshot(&reference).err().unwrap().details().entity(),
            Some("model")
        );
    }

    #[test]
    fn invalid_keys_are_rejected() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        let id = store.list().unwrap()[0].endpoints[0].keys[0].id.clone();
        let cases = [
            (json!({ "endpointId": ep }), ("apiKey", "empty")),
            (
                json!({ "endpointId": ep, "key": "  " }),
                ("apiKey", "empty"),
            ),
            (
                json!({ "endpointId": ep, "id": id, "key": "" }),
                ("apiKey", "empty"),
            ),
            (
                json!({ "endpointId": ep, "key": "k".repeat(4097) }),
                ("apiKey", "too_long"),
            ),
            (
                json!({ "endpointId": ep, "key": format!("{KEY} ") }),
                ("apiKey", "duplicate"),
            ),
            (
                json!({ "endpointId": ep, "key": "k2", "label": "x".repeat(41) }),
                ("label", "too_long"),
            ),
        ];
        for (input, (field, why)) in cases {
            let error = save_key(&store, input).unwrap_err();
            assert_eq!(
                reason(&error),
                (KitErrorCode::BadRequest, Some(field), Some(why))
            );
            assert_eq!(store.reveal_key(&id).unwrap().as_str(), KEY);
            assert_eq!(keys_of(&store, &ep), [KEY]);
        }
        // Re-saving a key's own value is not a duplicate of itself.
        save_key(&store, json!({ "endpointId": ep, "id": id, "key": KEY })).unwrap();
        let missing = save_key(&store, json!({ "endpointId": "nope", "key": "k2" })).unwrap_err();
        assert_eq!(missing.details().entity(), Some("endpoint"));
        let missing = save_key(
            &store,
            json!({ "endpointId": ep, "id": "nope", "label": "x" }),
        )
        .unwrap_err();
        assert_eq!(missing.details().entity(), Some("key"));
        for n in 1..16 {
            save_key(&store, json!({ "endpointId": ep, "key": format!("k-{n}") })).unwrap();
        }
        let error = save_key(&store, json!({ "endpointId": ep, "key": "k-17" })).unwrap_err();
        assert_eq!(
            reason(&error),
            (KitErrorCode::BadRequest, Some("keys"), Some("too_many"))
        );
        assert_eq!(store.list().unwrap()[0].endpoints[0].keys.len(), 16);
    }

    #[test]
    fn deleting_endpoints_and_providers_removes_their_keys() {
        let (_dir, store) = store();
        let count = |store: &Store| -> i64 {
            store
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM endpoint_keys", [], |row| row.get(0))
                .unwrap()
        };
        let (provider, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        store
            .save(request(
                json!({ "id": provider, "name": "P", "endpoints": [endpoint(None, "b", &[])] }),
            ))
            .unwrap();
        assert_eq!(count(&store), 0);
        let (provider, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        store.delete(&provider).unwrap();
        assert_eq!(count(&store), 0);
    }

    #[test]
    fn empty_address_saves_but_does_not_resolve() {
        let (_dir, store) = store();
        let mut ep = endpoint(None, "a", &["m1"]);
        ep["baseUrl"] = json!("");
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [ep] })))
            .unwrap();
        let reference = ModelRef {
            endpoint_id: saved[0].endpoints[0].id.clone(),
            model_id: "m1".to_owned(),
        };
        let error = store.resolve(&reference).unwrap_err();
        assert_eq!(
            reason(&error),
            (KitErrorCode::NeedsConfig, None, Some("missing_url"))
        );
    }

    #[test]
    fn unreadable_ciphertext_is_reported_not_fatal() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        // Unknown format marker, as after copying the database to another platform.
        store
            .lock()
            .unwrap()
            .execute("UPDATE endpoint_keys SET key_ciphertext = X'7F0102'", [])
            .unwrap();
        let key = &store.list().unwrap()[0].endpoints[0].keys[0];
        assert!(!key.readable && key.masked.is_none());
        let error = store.endpoint_connection(&ep).err().unwrap();
        assert_eq!(
            reason(&error),
            (KitErrorCode::NeedsConfig, None, Some("key_unreadable"))
        );
    }

    #[test]
    fn delete_and_model_options() {
        let (_dir, store) = store();
        let mut ep = endpoint(None, "a", &["gpt", "embed"]);
        ep["models"][1]["modelType"] = json!("embedding");
        ep["models"][1]["supportsTools"] = json!(null);
        let saved = store
            .save(request(json!({ "name": "Acme", "endpoints": [ep] })))
            .unwrap();
        let options = store
            .model_options(ModelFilter {
                model_type: None,
                supports_tools: Some(true),
            })
            .unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].label, "Acme · a · gpt");
        let embeddings = ModelFilter {
            model_type: Some(ModelType::Embedding),
            supports_tools: None,
        };
        assert_eq!(
            store.model_options(embeddings).unwrap()[0].model_id,
            "embed"
        );

        assert!(store.delete(&saved[0].id).unwrap().is_empty());
        assert!(
            store
                .model_options(ModelFilter::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.delete(&saved[0].id).err().unwrap().code(),
            KitErrorCode::NotFound
        );
    }

    #[test]
    fn invalid_input_is_rejected() {
        let (_dir, store) = store();
        let cases = [
            (json!({ "name": " ", "endpoints": [] }), "name"),
            (
                json!({ "name": "P", "endpoints": [endpoint(None, "Bad_Slug", &[])] }),
                "slug",
            ),
            (
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &[]), endpoint(None, "a", &[])] }),
                "slug",
            ),
            (
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &["m", "m"])] }),
                "models",
            ),
        ];
        for (input, field) in cases {
            let error = store.save(request(input)).err().unwrap();
            assert_eq!(
                (error.code(), error.details().field()),
                (KitErrorCode::BadRequest, Some(field))
            );
        }
        let mut ftp = endpoint(None, "a", &[]);
        ftp["baseUrl"] = json!("ftp://x");
        let error = store
            .save(request(json!({ "name": "P", "endpoints": [ftp] })))
            .err()
            .unwrap();
        assert_eq!(error.details().field(), Some("baseUrl"));
        assert!(store.list().unwrap().is_empty());
        let unknown = json!({ "name": "P", "endpoints": [], "roles": [] });
        assert!(serde_json::from_value::<ProviderSave>(unknown).is_err());
    }

    #[test]
    fn version_five_db_upgrades_with_empty_request_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("providers.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, position INTEGER NOT NULL, UNIQUE(provider_id, slug));
             CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, native_web_search INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint_id, model_id));
             CREATE TABLE endpoint_keys (id TEXT PRIMARY KEY, endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, label TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1, key_ciphertext BLOB NOT NULL, position INTEGER NOT NULL);
             INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1, 1);
             INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-chat', 0);
             PRAGMA user_version = 5;",
        )
        .unwrap();
        drop(db);
        let store = Store::open(dir.path()).unwrap();
        let endpoint = &store.list().unwrap()[0].endpoints[0];
        assert_eq!(endpoint.image_base_url, None);
        assert_eq!(endpoint.image_edit_base_url, None);
        assert!(endpoint.headers.is_empty());
        let connection = store.endpoint_connection("e").unwrap();
        assert!(connection.headers.is_empty() && connection.image_base_url.is_none());
        let version: u32 = store
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
    }

    #[test]
    fn version_six_db_upgrades_to_seven_keeping_request_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("providers.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, position INTEGER NOT NULL, image_base_url TEXT, headers TEXT NOT NULL DEFAULT '{}', UNIQUE(provider_id, slug));
             CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, native_web_search INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint_id, model_id));
             CREATE TABLE endpoint_keys (id TEXT PRIMARY KEY, endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, label TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1, key_ciphertext BLOB NOT NULL, position INTEGER NOT NULL);
             CREATE INDEX idx_endpoint_keys ON endpoint_keys(endpoint_id, position, id);
             INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1, 1);
             INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-chat', 0, 'https://img.example.com', '{\"X-A\":\"1\"}');
             PRAGMA user_version = 6;",
        )
        .unwrap();
        drop(db);
        let store = Store::open(dir.path()).unwrap();
        let endpoint = &store.list().unwrap()[0].endpoints[0];
        assert_eq!(
            endpoint.image_base_url.as_deref(),
            Some("https://img.example.com")
        );
        assert_eq!(endpoint.image_edit_base_url, None);
        assert_eq!(endpoint.headers.len(), 1);
        assert!(
            store
                .endpoint_connection("e")
                .unwrap()
                .image_edit_base_url
                .is_none()
        );
        let version: u32 = store
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
        assert_eq!(store.list().unwrap()[0].notes, "");
    }

    #[test]
    fn version_nine_db_upgrades_to_ten_with_empty_display_and_pricing() {
        let (dir, store) = store();
        let (_, endpoint_id) = saved(&store);
        store
            .lock()
            .unwrap()
            .execute_batch(
                "ALTER TABLE models DROP COLUMN name; ALTER TABLE models DROP COLUMN group_name;
                 ALTER TABLE models DROP COLUMN pricing; PRAGMA user_version = 9;",
            )
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let version: u32 = store
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
        let model = &store.list().unwrap()[0].endpoints[0].models[0];
        assert_eq!(
            (
                model.id.as_str(),
                &model.name,
                &model.group_name,
                &model.pricing
            ),
            ("m1", &None, &None, &None)
        );
        let reference = ModelRef {
            endpoint_id,
            model_id: "m1".into(),
        };
        assert!(store.resolve(&reference).unwrap().pricing.is_none());
    }

    #[test]
    fn display_fields_and_pricing_round_trip_to_every_read_path() {
        let (dir, store) = store();
        let mut e = endpoint(None, "a", &["m1"]);
        let pricing = json!({ "currency": "CNY", "tiers": [
            { "minInputTokens": 0, "input": 2.0, "output": 8.0, "cacheRead": 0.5, "cacheWrite": null },
            { "minInputTokens": 32000, "input": 4.0, "output": 16.0, "cacheRead": null, "cacheWrite": null }
        ] });
        e["models"][0]["name"] = json!("  Model One  ");
        e["models"][0]["groupName"] = json!("   ");
        e["models"][0]["pricing"] = pricing.clone();
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [e] })))
            .unwrap();
        let reference = ModelRef {
            endpoint_id: saved[0].endpoints[0].id.clone(),
            model_id: "m1".into(),
        };
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let model = &store.list().unwrap()[0].endpoints[0].models[0];
        assert_eq!(model.name.as_deref(), Some("Model One"));
        assert_eq!(model.group_name, None);
        assert_eq!(serde_json::to_value(&model.pricing).unwrap(), pricing);
        let option = &store.model_options(ModelFilter::default()).unwrap()[0];
        assert_eq!(option.name.as_deref(), Some("Model One"));
        assert_eq!(option.label, "P · a · m1");
        assert_eq!(
            serde_json::to_value(store.resolve(&reference).unwrap().pricing).unwrap(),
            pricing
        );
    }

    #[test]
    fn null_reasoning_turns_chat_reasoning_off_without_reinference() {
        let (dir, store) = store();
        let mut e = endpoint(None, "a", &["claude-sonnet-5-5", "embed"]);
        e["protocol"] = json!("anthropic-messages");
        e["models"][0]["reasoning"] = Value::Null;
        e["models"][1]["modelType"] = json!("embedding");
        e["models"][1]["reasoning"] = Value::Null;
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [e.clone()] })))
            .unwrap();
        let id = saved[0].endpoints[0].id.clone();
        let stored = |store: &Store| -> Vec<Option<String>> {
            store
                .lock()
                .unwrap()
                .prepare("SELECT reasoning FROM models ORDER BY position")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(stored(&store), vec![Some("none".to_owned()), None]);
        // Omitted keeps the stored 'none'.
        e["id"] = json!(id);
        for model in e["models"].as_array_mut().unwrap() {
            model.as_object_mut().unwrap().remove("reasoning");
        }
        store
            .save(request(
                json!({ "id": saved[0].id, "name": "P", "endpoints": [e] }),
            ))
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(stored(&store), vec![Some("none".to_owned()), None]);
        let reference = ModelRef {
            endpoint_id: id,
            model_id: "claude-sonnet-5-5".into(),
        };
        assert!(
            store.list().unwrap()[0].endpoints[0].models[0]
                .reasoning
                .is_none()
        );
        assert!(store.resolve(&reference).unwrap().reasoning.is_none());
        assert!(
            store.model_options(ModelFilter::default()).unwrap()[0]
                .reasoning
                .is_none()
        );
    }

    #[test]
    fn kept_reasoning_is_revalidated_against_the_new_protocol_and_output() {
        let (_dir, store) = store();
        let mut e = endpoint(None, "a", &["m1"]);
        e["protocol"] = json!("anthropic-messages");
        e["models"][0]["maxOutput"] = json!(8000);
        e["models"][0]["reasoning"] =
            json!({ "dialect": "anthropic-budget", "efforts": ["high"], "budgetTokens": 4000 });
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [e.clone()] })))
            .unwrap();
        e["id"] = json!(saved[0].endpoints[0].id);
        e["models"][0].as_object_mut().unwrap().remove("reasoning");
        let resave = |e: &Value| {
            store
                .save(request(
                    json!({ "id": saved[0].id, "name": "P", "endpoints": [e] }),
                ))
                .err()
                .map(|error| reason(&error).2.map(str::to_owned))
        };
        let mut lower = e.clone();
        lower["models"][0]["maxOutput"] = json!(4000);
        assert_eq!(resave(&lower), Some(Some("invalid_budget".to_owned())));
        let mut openai = e.clone();
        openai["protocol"] = json!("openai-chat");
        assert_eq!(resave(&openai), Some(Some("protocol_mismatch".to_owned())));
        let mut image = e.clone();
        image["models"][0]["modelType"] = json!("image");
        assert_eq!(resave(&image), Some(Some("invalid_efforts".to_owned())));
        assert_eq!(resave(&e), None);
    }

    #[test]
    fn invalid_model_settings_are_rejected_with_contract_reasons() {
        let (_dir, store) = store();
        let tier = |min: u64, price: f64| json!({ "minInputTokens": min, "input": price, "output": 1, "cacheRead": null, "cacheWrite": null });
        let pricing = |tiers: Vec<Value>| json!({ "currency": "USD", "tiers": tiers });
        let long = "x".repeat(81);
        let cases = [
            (json!({ "name": long }), "openai-chat", "name", "too_long"),
            (
                json!({ "groupName": long }),
                "openai-chat",
                "groupName",
                "too_long",
            ),
            (
                json!({ "contextWindow": 0 }),
                "openai-chat",
                "contextWindow",
                "invalid",
            ),
            (
                json!({ "maxOutput": 0 }),
                "openai-chat",
                "maxOutput",
                "invalid",
            ),
            (
                json!({ "maxOutput": 200_000 }),
                "openai-chat",
                "maxOutput",
                "exceeds_context",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": [] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": ["low", "low"] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": ["default"] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "modelType": "image", "reasoning": { "dialect": "openai", "efforts": ["low"] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-adaptive", "efforts": ["low"] } }),
                "openai-chat",
                "reasoning",
                "protocol_mismatch",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": ["low"] } }),
                "anthropic-messages",
                "reasoning",
                "protocol_mismatch",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-budget", "efforts": ["low"] } }),
                "anthropic-messages",
                "reasoning",
                "invalid_budget",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-budget", "efforts": ["low"], "budgetTokens": 1023 } }),
                "anthropic-messages",
                "reasoning",
                "invalid_budget",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-budget", "efforts": ["low"], "budgetTokens": 4096 } }),
                "anthropic-messages",
                "reasoning",
                "invalid_budget",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-adaptive", "efforts": ["low"], "budgetTokens": 2000 } }),
                "anthropic-messages",
                "reasoning",
                "unexpected_budget",
            ),
            (
                json!({ "modelType": "image", "pricing": pricing(vec![tier(0, 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing((0..9).map(|i| tier(i * 10, 1.0)).collect()) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(5, 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(0, 1.0), tier(0, 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(0, -1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(0, 1e6 + 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
        ];
        for (extra, protocol, field, why) in cases {
            let mut value = endpoint(None, "a", &["m1"]);
            value["protocol"] = json!(protocol);
            for (k, v) in extra.as_object().unwrap() {
                value["models"][0][k] = v.clone();
            }
            let error = store
                .save(request(json!({ "name": "P", "endpoints": [value] })))
                .err()
                .unwrap();
            assert_eq!(
                reason(&error),
                (KitErrorCode::BadRequest, Some(field), Some(why)),
                "{extra}"
            );
        }
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn presets_seed_once_adopt_same_name_and_cannot_be_deleted() {
        let (_dir, store) = store();
        let seed = |name: &str, preset: &str| {
            store.save(request(
                json!({ "name": name, "presetId": preset, "enabled": false,
                "endpoints": [endpoint(None, "ep-1", &[])] }),
            ))
        };
        let seeded = seed("七牛云", "qiniu").unwrap();
        assert_eq!(seeded.len(), 1);
        assert_eq!(seeded[0].preset_id.as_deref(), Some("qiniu"));
        assert!(!seeded[0].enabled);
        assert_eq!(seed("七牛云", "qiniu").unwrap().len(), 1);
        let id = seeded[0].id.clone();
        let renamed = store
            .save(request(
                json!({ "id": id, "name": "Q", "presetId": "other", "endpoints": [] }),
            ))
            .unwrap();
        assert_eq!(renamed[0].preset_id.as_deref(), Some("qiniu"));
        let error = store.delete(&id).unwrap_err();
        assert_eq!(
            reason(&error),
            (KitErrorCode::Conflict, None, Some("preset_provider"))
        );
        assert_eq!(store.list().unwrap().len(), 1);

        let own = store
            .save(request(
                json!({ "name": "Fenno", "endpoints": [endpoint(None, "mine", &[])] }),
            ))
            .unwrap();
        let fenno = own.iter().find(|p| p.name == "Fenno").unwrap().id.clone();
        let adopted = seed("fenno", "fenno").unwrap();
        assert_eq!(adopted.len(), 2);
        let fenno = adopted.iter().find(|p| p.id == fenno).unwrap();
        assert_eq!(fenno.preset_id.as_deref(), Some("fenno"));
        assert_eq!(fenno.endpoints[0].slug, "mine");
    }

    #[test]
    fn notes_save_keep_when_omitted_and_reject_oversized() {
        let (_dir, store) = store();
        let saved = store
            .save(request(
                json!({ "name": "P", "notes": "## 价格\n- 便宜", "endpoints": [] }),
            ))
            .unwrap();
        let id = saved[0].id.clone();
        assert_eq!(saved[0].notes, "## 价格\n- 便宜");
        let kept = store
            .save(request(json!({ "id": id, "name": "P2", "endpoints": [] })))
            .unwrap();
        assert_eq!(kept[0].notes, "## 价格\n- 便宜");
        let cleared = store
            .save(request(
                json!({ "id": id, "name": "P2", "notes": "", "endpoints": [] }),
            ))
            .unwrap();
        assert_eq!(cleared[0].notes, "");
        let error = store
            .save(request(
                json!({ "id": id, "name": "P2", "notes": "x".repeat(10_001), "endpoints": [] }),
            ))
            .unwrap_err();
        assert_eq!(reason(&error).1, Some("notes"));
    }

    #[test]
    fn request_config_round_trips_and_omitted_fields_keep() {
        let (_dir, store) = store();
        let mut first = endpoint(None, "a", &["m1"]);
        first["imageBaseUrl"] = json!("  https://img.example.com/  ");
        first["imageEditBaseUrl"] = json!(" https://edit.example.com ");
        first["headers"] = json!({ " X-Relay-Tag ": "team-a", "X-Two": "2" });
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [first] })))
            .unwrap();
        let (provider, ep) = (saved[0].id.clone(), saved[0].endpoints[0].id.clone());
        let expected = BTreeMap::from([
            ("X-Relay-Tag".to_owned(), "team-a".to_owned()),
            ("X-Two".to_owned(), "2".to_owned()),
        ]);
        let check = |store: &Store| {
            let listed = &store.list().unwrap()[0].endpoints[0];
            assert_eq!(
                listed.image_base_url.as_deref(),
                Some("https://img.example.com/")
            );
            assert_eq!(
                listed.image_edit_base_url.as_deref(),
                Some("https://edit.example.com")
            );
            assert_eq!(listed.headers, expected);
            let resolved = store
                .resolve(&ModelRef {
                    endpoint_id: ep.clone(),
                    model_id: "m1".to_owned(),
                })
                .unwrap();
            assert_eq!(
                resolved.image_base_url.as_deref(),
                Some("https://img.example.com/v1")
            );
            assert_eq!(
                resolved.image_edit_base_url.as_deref(),
                Some("https://edit.example.com/v1")
            );
            assert_eq!(resolved.headers.len(), 2);
        };
        check(&store);
        // Omitted = keep.
        let kept = endpoint(Some(&ep), "a", &["m1"]);
        store
            .save(request(
                json!({ "id": provider, "name": "P", "endpoints": [kept] }),
            ))
            .unwrap();
        check(&store);
        // null / "" and {} clear.
        for image in [json!(null), json!("  ")] {
            let mut cleared = endpoint(Some(&ep), "a", &["m1"]);
            cleared["imageBaseUrl"] = image.clone();
            cleared["imageEditBaseUrl"] = image;
            cleared["headers"] = json!({});
            let listed = store
                .save(request(
                    json!({ "id": provider, "name": "P", "endpoints": [cleared] }),
                ))
                .unwrap();
            assert_eq!(listed[0].endpoints[0].image_base_url, None);
            assert_eq!(listed[0].endpoints[0].image_edit_base_url, None);
            assert!(listed[0].endpoints[0].headers.is_empty());
        }
    }

    #[test]
    fn invalid_request_config_is_rejected() {
        let (_dir, store) = store();
        let many: serde_json::Map<String, Value> =
            (0..21).map(|i| (format!("x-{i}"), json!("v"))).collect();
        let cases = [
            (
                json!({ "imageBaseUrl": "ftp://x" }),
                "imageBaseUrl",
                "invalid_url",
            ),
            (
                json!({ "imageBaseUrl": format!("https://{}", "a".repeat(2048)) }),
                "imageBaseUrl",
                "invalid_url",
            ),
            (
                json!({ "imageEditBaseUrl": "ftp://x" }),
                "imageEditBaseUrl",
                "invalid_url",
            ),
            (
                json!({ "imageEditBaseUrl": format!("https://{}", "a".repeat(2048)) }),
                "imageEditBaseUrl",
                "invalid_url",
            ),
            (json!({ "headers": many }), "headers", "too_many"),
            (
                json!({ "headers": { "bad name": "v" } }),
                "headers",
                "invalid",
            ),
            (json!({ "headers": { " ": "v" } }), "headers", "invalid"),
            (
                json!({ "headers": { "X-A": "a\r\nb" } }),
                "headers",
                "invalid",
            ),
            (
                json!({ "headers": { "X-A": "v".repeat(2049) } }),
                "headers",
                "invalid",
            ),
            (
                json!({ "headers": { "X-A": "1", "x-a": "2" } }),
                "headers",
                "invalid",
            ),
            (
                json!({ "headers": { "Authorization": "Bearer x" } }),
                "headers",
                "reserved",
            ),
            (
                json!({ "headers": { "X-API-Key": "x" } }),
                "headers",
                "reserved",
            ),
            (
                json!({ "headers": { "cookie": "x" } }),
                "headers",
                "reserved",
            ),
        ];
        for (extra, field, why) in cases {
            let mut value = endpoint(None, "a", &[]);
            for (k, v) in extra.as_object().unwrap() {
                value[k] = v.clone();
            }
            let error = store
                .save(request(json!({ "name": "P", "endpoints": [value] })))
                .err()
                .unwrap();
            assert_eq!(
                reason(&error),
                (KitErrorCode::BadRequest, Some(field), Some(why)),
                "{extra}"
            );
        }
        assert!(store.list().unwrap().is_empty());
    }