//! Store by turn: trimming, crash leftovers, regenerate, reopen (docs/12 §6).
#![expect(clippy::unwrap_used, reason = "test assertions")]

use aether_support::{KitErrorCode, SkillRef};
use serde_json::json;
use tempfile::TempDir;

use super::{ConversationStore, SqliteStore, Turn, TurnRecord, TurnStatus, trim_history};
use crate::types::{EndStatus, Message, ToolCall};

fn completed(messages: Vec<Message>) -> TurnRecord {
    TurnRecord {
        status: EndStatus::Completed,
        messages,
        model_id: Some("m1".into()),
        skills: vec![SkillRef {
            id: "s1".into(),
            revision: 2,
        }],
        error_code: None,
        input_tokens: Some(10),
        output_tokens: Some(5),
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        context_usage: None,
        reasoning_effort: None,
        endpoint_id: None,
    }
}

fn tool_turn(seq: u32, input: &str) -> Turn {
    Turn {
        id: format!("t{seq}"),
        conversation_id: "conversation".into(),
        parent_id: (seq > 1).then(|| format!("t{}", seq - 1)),
        question_id: format!("t{seq}"),
        supersedes_id: None,
        kind: "reply".into(),
        seq,
        status: TurnStatus::Completed,
        input: input.into(),
        messages: vec![
            Message::Assistant {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: format!("c{seq}"),
                    name: "search".into(),
                    args: json!({"q": "x"}),
                }],
                reasoning: vec![],
            },
            Message::Tool {
                call_id: format!("c{seq}"),
                text: "result".repeat(10),
            },
            Message::Assistant {
                text: "done".into(),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ],
        model_id: None,
        skills: vec![],
        error_code: None,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        context_usage: None,
        reasoning_effort: None,
        endpoint_id: None,
        created_at: 0,
    }
}

/// Every tool result has its call and every call has its result.
fn assert_paired(history: &[Message]) {
    let calls: Vec<&str> = history
        .iter()
        .flat_map(|message| match message {
            Message::Assistant { tool_calls, .. } => {
                tool_calls.iter().map(|call| call.id.as_str()).collect()
            }
            Message::User(_) | Message::Tool { .. } => vec![],
        })
        .collect();
    let results: Vec<&str> = history
        .iter()
        .filter_map(|message| match message {
            Message::Tool { call_id, .. } => Some(call_id.as_str()),
            Message::User(_) | Message::Assistant { .. } => None,
        })
        .collect();
    assert_eq!(calls, results);
}

#[test]
fn trimming_drops_whole_turns_from_the_oldest() {
    let mut failed = tool_turn(3, "failed one");
    failed.status = TurnStatus::Failed;
    let turns = [
        tool_turn(1, "first"),
        tool_turn(2, "second"),
        failed,
        tool_turn(4, "fourth"),
    ];
    let per_turn = trim_history(&turns[3..], u32::MAX).len();
    assert_eq!(per_turn, 4); // user + assistant(call) + tool + assistant

    let all = trim_history(&turns, u32::MAX);
    assert_eq!(all.len(), 3 * per_turn, "failed turn never enters history");
    assert_paired(&all);

    // Budget for exactly the two newest completed turns: the oldest is dropped whole.
    let chars: usize = trim_history(&turns[3..], u32::MAX)
        .iter()
        .map(super::message_chars)
        .sum();
    let two = u32::try_from((2 * chars).div_ceil(3)).unwrap();
    let kept = trim_history(&turns, two);
    assert_eq!(kept.len(), 2 * per_turn);
    assert_eq!(kept[0], Message::User("second".into()));
    assert_paired(&kept);

    // One token short of the newest turn: no history at all, never half a turn.
    let one = u32::try_from(chars.div_ceil(3)).unwrap();
    assert_eq!(trim_history(&turns, one).len(), per_turn);
    assert!(trim_history(&turns, one - 1).is_empty());
}

#[test]
fn reopen_reads_history_back() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = store
        .begin_turn(
            None,
            Some("doc-1"),
            "line one\r\nline two that is long enough to be cut at forty",
        )
        .unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first, completed(tool_turn(1, "").messages))
        .unwrap();
    let second = store.begin_turn(Some(&id), None, "again").unwrap();
    let partial = vec![Message::Assistant {
        text: "部分".into(),
        tool_calls: vec![],
        reasoning: vec![],
    }];
    store
        .finish_turn(
            second,
            TurnRecord {
                status: EndStatus::Cancelled,
                messages: partial.clone(),
                ..completed(vec![])
            },
        )
        .unwrap();
    drop(store);

    let store = SqliteStore::open(dir.path()).unwrap();
    let conversation = store.load(&id).unwrap();
    assert_eq!(
        conversation.summary.title,
        "line oneline two that is long enough to "
    );
    assert_eq!(conversation.summary.scope.as_deref(), Some("doc-1"));
    assert_eq!(conversation.turns.len(), 2);
    let (one, two) = (&conversation.turns[0], &conversation.turns[1]);
    assert_eq!((one.seq, one.status), (1, TurnStatus::Completed));
    assert_eq!(one.messages, tool_turn(1, "").messages);
    assert_eq!(
        one.skills,
        vec![SkillRef {
            id: "s1".into(),
            revision: 2
        }]
    );
    assert_eq!(
        (one.model_id.as_deref(), one.input_tokens, one.output_tokens),
        (Some("m1"), Some(10), Some(5))
    );
    assert_eq!(
        (two.seq, two.status, two.input.as_str()),
        (2, TurnStatus::Cancelled, "again")
    );
    assert_eq!(
        two.messages, partial,
        "cancelled turn keeps partial text for display"
    );

    assert_eq!(store.list(Some("doc-1"), 10).unwrap().len(), 1);
    assert!(store.list(Some("other"), 10).unwrap().is_empty());
    assert_eq!(store.rename(&id, "  新标题 ").unwrap().title, "新标题");
    assert_eq!(
        store.rename(&id, " ").unwrap_err().code(),
        KitErrorCode::BadRequest
    );
    store.delete(&id).unwrap();
    assert_eq!(store.load(&id).unwrap_err().code(), KitErrorCode::NotFound);
    assert_eq!(
        store.begin_turn(Some(&id), None, "x").unwrap_err().code(),
        KitErrorCode::NotFound
    );
}

#[test]
fn running_turn_left_by_a_crash_becomes_failed() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let handle = store.begin_turn(None, None, "hi").unwrap();
    assert_eq!(
        store.load(&handle.conversation_id).unwrap().turns[0].status,
        TurnStatus::Running
    );
    drop(store); // crash: finish_turn never ran

    let store = SqliteStore::open(dir.path()).unwrap();
    let turn = &store.load(&handle.conversation_id).unwrap().turns[0];
    assert_eq!(turn.status, TurnStatus::Failed);
    assert_eq!(turn.error_code.as_deref(), Some("interrupted"));
    // The stale handle cannot overwrite the converted turn.
    let error = store.finish_turn(handle, completed(vec![])).unwrap_err();
    assert_eq!(error.code(), KitErrorCode::NotFound);
}

#[test]
fn regenerate_keeps_source_and_selection_across_restart() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = store.begin_turn(None, None, "q1").unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first.clone(), completed(tool_turn(1, "").messages))
        .unwrap();
    let second = store.begin_turn(Some(&id), None, "q2").unwrap();
    assert_eq!(
        store.regenerate(&id).unwrap_err().details().reason(),
        Some("turn_running")
    );
    store
        .finish_turn(second.clone(), completed(tool_turn(2, "").messages))
        .unwrap();
    let before = store.load(&id).unwrap();
    let again = store.regenerate(&id).unwrap();
    assert_ne!(again.turn_id, second.turn_id);
    assert_eq!(again.input, "q2");
    assert_eq!(again.parent_id, Some(first.turn_id.clone()));
    assert_eq!(again.question_id, second.question_id);
    assert_eq!(again.supersedes_id, Some(second.turn_id.clone()));
    let running = store.load(&id).unwrap();
    assert_eq!(running.turns.len(), 3);
    assert_eq!(running.turns[..2], before.turns);
    assert_eq!(
        running.summary.selected_turn_id,
        Some(second.turn_id.clone())
    );
    store
        .finish_turn(again.clone(), completed(answer("candidate")))
        .unwrap();
    let selected = store.load(&id).unwrap();
    assert_eq!(
        selected.summary.selected_turn_id,
        Some(second.turn_id.clone())
    );
    assert_eq!(
        super::history_path(&selected.turns, Some(&second.turn_id))
            .unwrap()
            .len(),
        2
    );
    drop(store);
    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(store.load(&id).unwrap(), selected);
    store.select_turn(&id, &again.turn_id).unwrap();
    let next = store.begin_turn(Some(&id), None, "q3").unwrap();
    assert_eq!(next.parent_id, Some(again.turn_id.clone()));
    let turns = store.load(&id).unwrap().turns;
    let path = super::history_path(&turns, next.parent_id.as_deref()).unwrap();
    assert_eq!(
        path.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec![first.turn_id.as_str(), again.turn_id.as_str()]
    );
    let history = trim_history(&path, u32::MAX);
    assert!(history.contains(&answer("candidate")[0]));
    assert_paired(&history);
    store.finish_turn(next, completed(answer("third"))).unwrap();
    store.delete(&id).unwrap();
    assert_eq!(store.load(&id).unwrap_err().code(), KitErrorCode::NotFound);
}

fn answer(text: &str) -> Vec<Message> {
    vec![Message::Assistant {
        text: text.into(),
        tool_calls: vec![],
        reasoning: vec![],
    }]
}

#[test]
fn edit_compare_and_swap_preserves_usage_and_invalidates_native_context() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let handle = store.begin_turn(None, None, "question").unwrap();
    let mut messages = tool_turn(1, "").messages;
    messages.push(answer("tail").remove(0));
    let mut record = completed(messages);
    record.endpoint_id = Some("endpoint".into());
    record.cache_read_tokens = Some(7);
    record.cache_write_tokens = Some(3);
    record.reasoning_tokens = Some(2);
    record.reasoning_effort = Some(aether_support::ReasoningEffort::High);
    record.context_usage = Some(aether_support::ContextUsage {
        tokens: Some(15),
        context_window: Some(4096),
        source: aether_support::ContextSource::Reported,
        endpoint_id: record.endpoint_id.clone(),
        model_id: record.model_id.clone(),
    });
    store.finish_turn(handle.clone(), record).unwrap();
    let id = &handle.conversation_id;
    let before = store.load(id).unwrap();
    assert_eq!(
        store
            .edit_turn(id, &handle.turn_id, "changed", "done")
            .unwrap_err()
            .details()
            .reason(),
        Some("text_changed")
    );
    assert_eq!(store.load(id).unwrap(), before);
    let after = store
        .edit_turn(id, &handle.turn_id, "**changed**", "done\n\ntail")
        .unwrap();
    let expected = Turn {
        messages: answer("**changed**"),
        context_usage: None,
        ..before.turns[0].clone()
    };
    assert_eq!(after.turns[0], expected);
    assert_eq!(
        after.summary.selected_turn_id,
        before.summary.selected_turn_id
    );
    assert_eq!(
        store
            .edit_turn(id, &handle.turn_id, " ", "**changed**")
            .unwrap_err()
            .code(),
        KitErrorCode::BadRequest
    );
    assert_eq!(
        store
            .edit_turn(id, &handle.turn_id, &"x".repeat(24_001), "**changed**")
            .unwrap_err()
            .code(),
        KitErrorCode::BadRequest
    );
    assert_eq!(
        store
            .edit_turn(id, &handle.turn_id, "newer", "done\n\ntail")
            .unwrap_err()
            .details()
            .reason(),
        Some("text_changed")
    );
}

#[test]
fn running_foreign_selected_and_dependent_targets_are_rejected_atomically() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = store.begin_turn(None, None, "first").unwrap();
    let id = &first.conversation_id;
    store
        .finish_turn(first.clone(), completed(answer("first reply")))
        .unwrap();
    let foreign = store.begin_turn(None, None, "foreign").unwrap();
    store
        .finish_turn(foreign.clone(), completed(answer("foreign reply")))
        .unwrap();
    for result in [
        store.edit_turn(id, &foreign.turn_id, "edit", "foreign reply"),
        store.remove_turn(id, &foreign.turn_id),
        store.select_turn(id, &foreign.turn_id),
        store.fork(id, &foreign.turn_id),
    ] {
        assert_eq!(result.unwrap_err().code(), KitErrorCode::NotFound);
    }
    for (parent, retry, translation, language) in [
        (Some(foreign.turn_id.as_str()), None, None, None),
        (None, Some(foreign.turn_id.as_str()), None, None),
        (None, None, Some(foreign.turn_id.as_str()), Some("French")),
    ] {
        assert_eq!(
            store
                .begin_action(
                    Some(id),
                    None,
                    "ignored",
                    parent,
                    retry,
                    translation,
                    language
                )
                .unwrap_err()
                .code(),
            KitErrorCode::NotFound
        );
    }
    assert_eq!(
        store
            .remove_turn(id, &first.turn_id)
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_selected")
    );
    let candidate = store.regenerate(id).unwrap();
    let before = store.load(id).unwrap();
    for result in [
        store.edit_turn(id, &first.turn_id, "edit", "first reply"),
        store.remove_turn(id, &candidate.turn_id),
        store.select_turn(id, &first.turn_id),
        store.fork(id, &first.turn_id),
    ] {
        assert_eq!(result.unwrap_err().details().reason(), Some("turn_running"));
    }
    assert_eq!(
        store
            .begin_turn(Some(id), None, "next")
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_running")
    );
    assert_eq!(store.load(id).unwrap(), before);
    store
        .finish_turn(candidate.clone(), completed(answer("candidate")))
        .unwrap();
    assert_eq!(
        store
            .edit_turn(id, &first.turn_id, "edit", "first reply")
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_has_dependents")
    );
    store.select_turn(id, &candidate.turn_id).unwrap();
    assert_eq!(
        store
            .remove_turn(id, &first.turn_id)
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_has_dependents")
    );
    store.select_turn(id, &first.turn_id).unwrap();
    let removed = store.remove_turn(id, &candidate.turn_id).unwrap();
    assert_eq!(removed.turns.len(), 1);
    assert_eq!(removed.turns[0].input, "first");
    let child = store.begin_turn(Some(id), None, "child").unwrap();
    store
        .finish_turn(child.clone(), completed(answer("child reply")))
        .unwrap();
    assert_eq!(
        store
            .edit_turn(id, &first.turn_id, "edit", "first reply")
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_has_dependents")
    );
    store.select_turn(id, &child.turn_id).unwrap();
    assert_eq!(
        store
            .remove_turn(id, &first.turn_id)
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_has_dependents")
    );
}

#[test]
fn terminal_failed_cancelled_replies_support_edit_retry_and_translation() {
    for status in [EndStatus::Failed, EndStatus::Cancelled] {
        let dir = TempDir::new().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let source = store
            .begin_turn(None, Some("scope"), "original input")
            .unwrap();
        let id = &source.conversation_id;
        store
            .finish_turn(
                source.clone(),
                TurnRecord {
                    status,
                    error_code: (status == EndStatus::Failed).then(|| "upstream".into()),
                    endpoint_id: Some("endpoint".into()),
                    ..completed(answer("visible partial"))
                },
            )
            .unwrap();
        assert_eq!(
            store
                .select_turn(id, &source.turn_id)
                .unwrap_err()
                .details()
                .reason(),
            Some("turn_not_completed")
        );
        store
            .edit_turn(id, &source.turn_id, "visible edited", "visible partial")
            .unwrap();
        let retry = store
            .begin_action(
                Some(id),
                None,
                "ignored",
                None,
                Some(&source.turn_id),
                None,
                None,
            )
            .unwrap();
        assert_eq!(retry.input, "original input");
        assert_eq!(retry.question_id, source.question_id);
        let running = store.load(id).unwrap();
        assert_eq!(running.turns[1].endpoint_id.as_deref(), Some("endpoint"));
        assert_eq!(running.turns[1].model_id.as_deref(), Some("m1"));
        store
            .finish_turn(
                retry.clone(),
                TurnRecord {
                    status,
                    ..completed(answer("another partial"))
                },
            )
            .unwrap();
        assert!(store.load(id).unwrap().summary.selected_turn_id.is_none());
        let translation = store
            .begin_action(
                Some(id),
                None,
                "ignored",
                None,
                None,
                Some(&source.turn_id),
                Some(" French "),
            )
            .unwrap();
        assert_eq!(translation.kind, "translation");
        assert_eq!(translation.parent_id, Some(source.turn_id.clone()));
        assert_eq!(translation.question_id, translation.turn_id);
        assert_eq!(translation.supersedes_id, None);
        assert_eq!(
            translation.input,
            "Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\nvisible edited"
        );
        store
            .finish_turn(translation.clone(), completed(answer("traduit")))
            .unwrap();
        assert_eq!(
            store.load(id).unwrap().summary.selected_turn_id,
            Some(translation.turn_id.clone())
        );
        let retry_translation = store.regenerate(id).unwrap();
        assert_eq!(retry_translation.kind, "translation");
        assert_eq!(retry_translation.question_id, translation.question_id);
        assert_eq!(retry_translation.parent_id, translation.parent_id);
        assert_eq!(
            retry_translation.supersedes_id,
            Some(translation.turn_id.clone())
        );
        store
            .finish_turn(retry_translation.clone(), completed(answer("autre")))
            .unwrap();
        assert_eq!(
            store.load(id).unwrap().summary.selected_turn_id,
            Some(translation.turn_id)
        );
        store.remove_turn(id, &retry_translation.turn_id).unwrap();
        store.remove_turn(id, &retry.turn_id).unwrap();
    }
}

#[test]
fn fork_copies_only_exact_ancestry_with_fresh_graph_ids_and_records() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let source = store
        .begin_turn(None, Some("note:scope"), "first question")
        .unwrap();
    let id = &source.conversation_id;
    store
        .finish_turn(source.clone(), completed(tool_turn(1, "").messages))
        .unwrap();
    let candidate = store.regenerate(id).unwrap();
    store
        .finish_turn(candidate.clone(), completed(answer("candidate")))
        .unwrap();
    store.select_turn(id, &candidate.turn_id).unwrap();
    let child = store.begin_turn(Some(id), None, "child input").unwrap();
    store
        .finish_turn(child.clone(), completed(tool_turn(2, "").messages))
        .unwrap();
    let tail = store.begin_turn(Some(id), None, "not copied").unwrap();
    store.finish_turn(tail, completed(answer("tail"))).unwrap();
    let original = store.load(id).unwrap();
    let fork = store.fork(id, &child.turn_id).unwrap();
    assert_ne!(fork.summary.id, *id);
    assert_eq!(fork.summary.scope, original.summary.scope);
    assert_eq!(fork.summary.title, original.summary.title);
    assert_eq!(fork.turns.len(), 2);
    assert_eq!(
        fork.summary.selected_turn_id.as_deref(),
        Some(fork.turns[1].id.as_str())
    );
    assert_eq!(fork.turns[0].parent_id, None);
    assert_eq!(
        fork.turns[0].supersedes_id, None,
        "source alternative is not part of the selected ancestry"
    );
    assert_eq!(
        fork.turns[1].parent_id.as_deref(),
        Some(fork.turns[0].id.as_str())
    );
    for (copied, source) in fork
        .turns
        .iter()
        .zip([&original.turns[1], &original.turns[2]])
    {
        assert!(
            !original
                .turns
                .iter()
                .any(|t| t.id == copied.id || t.question_id == copied.question_id)
        );
        assert_eq!(copied.conversation_id, fork.summary.id);
        assert_eq!(copied.messages, source.messages);
        assert_eq!(copied.input, source.input);
        assert_eq!(copied.model_id, source.model_id);
        assert_eq!(copied.input_tokens, source.input_tokens);
        assert_eq!(copied.output_tokens, source.output_tokens);
        assert_eq!(copied.created_at, source.created_at);
        assert_eq!(copied.kind, source.kind);
    }
    assert_eq!(store.load(id).unwrap(), original);
    let history = trim_history(
        &super::history_path(&fork.turns, fork.summary.selected_turn_id.as_deref()).unwrap(),
        u32::MAX,
    );
    assert_paired(&history);
    store.delete(&fork.summary.id).unwrap();
    assert_eq!(store.load(id).unwrap(), original);
}

#[test]
fn history_path_rejects_cycles_missing_nodes_and_cross_conversation_edges() {
    let first = tool_turn(1, "first");
    let mut second = tool_turn(2, "second");
    assert_eq!(
        super::history_path(&[first.clone(), second.clone()], Some("t2")).unwrap(),
        vec![first.clone(), second.clone()]
    );
    second.parent_id = Some("missing".into());
    assert_eq!(
        super::history_path(&[first.clone(), second.clone()], Some("t2"))
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    second.parent_id = Some("t2".into());
    assert_eq!(
        super::history_path(&[second.clone()], Some("t2"))
            .unwrap_err()
            .details()
            .reason(),
        Some("history_cycle")
    );
    second.parent_id = Some("t1".into());
    second.conversation_id = "foreign".into();
    assert_eq!(
        super::history_path(&[first, second], Some("t2"))
            .unwrap_err()
            .details()
            .reason(),
        Some("history_foreign_turn")
    );
}

#[test]
fn old_linear_schemas_migrate_incrementally_without_losing_records() {
    for version in [1, 2] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("chat.db");
        let old = aether_support::db::open(&path, &super::MIGRATIONS[..version]).unwrap();
        old.execute("INSERT INTO conversations (id,title,scope,created_at,updated_at) VALUES ('old','title','scope',100,200)", []).unwrap();
        for (seq, status) in [
            (1, "completed"),
            (2, "failed"),
            (3, "completed"),
            (4, "cancelled"),
            (5, "running"),
        ] {
            old.execute("INSERT INTO turns (id,conversation_id,seq,status,input,messages,model_id,skills,error_code,input_tokens,output_tokens,created_at) VALUES (?1,'old',?2,?3,?4,?5,'model','[]',?6,123,45,?7)", rusqlite::params![format!("old{seq}"),seq,status,format!("input{seq}"),serde_json::to_string(&tool_turn(seq, "").messages).unwrap(),(status == "failed").then_some("upstream"),seq*100]).unwrap();
        }
        if version == 2 {
            old.execute("UPDATE turns SET cache_read_tokens=9, cache_write_tokens=8, reasoning_tokens=7, endpoint_id='endpoint', reasoning_effort='\"high\"'", []).unwrap();
        }
        drop(old);
        let store = SqliteStore::open(dir.path()).unwrap();
        let c = store.load("old").unwrap();
        assert_eq!(c.summary.selected_turn_id.as_deref(), Some("old3"));
        assert_eq!(c.summary.scope.as_deref(), Some("scope"));
        assert_eq!((c.summary.created_at, c.summary.updated_at), (100, 200));
        assert_eq!(c.turns.len(), 5);
        for (index, turn) in c.turns.iter().enumerate() {
            let seq = u32::try_from(index + 1).unwrap();
            assert_eq!(turn.id, format!("old{seq}"));
            assert_eq!(turn.parent_id, (seq > 1).then(|| format!("old{}", seq - 1)));
            assert_eq!(turn.question_id, turn.id);
            assert_eq!(turn.supersedes_id, None);
            assert_eq!(turn.kind, "reply");
            assert_eq!(turn.messages, tool_turn(seq, "").messages);
            assert_eq!(turn.model_id.as_deref(), Some("model"));
            assert_eq!(
                (turn.input_tokens, turn.output_tokens),
                (Some(123), Some(45))
            );
            if version == 2 {
                assert_eq!(
                    (
                        turn.cache_read_tokens,
                        turn.cache_write_tokens,
                        turn.reasoning_tokens
                    ),
                    (Some(9), Some(8), Some(7))
                );
                assert_eq!(turn.endpoint_id.as_deref(), Some("endpoint"));
            }
        }
        assert_eq!(c.turns[4].status, TurnStatus::Failed);
        assert_eq!(c.turns[4].error_code.as_deref(), Some("interrupted"));
        assert!(dir.path().join(format!("chat.db.bak-v{version}")).exists());
        drop(store);
        let reopened = SqliteStore::open(dir.path()).unwrap();
        assert_eq!(reopened.load("old").unwrap(), c);
        let next = reopened.begin_turn(Some("old"), None, "next").unwrap();
        assert_eq!(next.parent_id.as_deref(), Some("old3"));
    }
}
