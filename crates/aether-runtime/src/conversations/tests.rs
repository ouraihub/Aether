//! Conversation persistence and reservation behavior (docs/26).
#![expect(clippy::unwrap_used, reason = "test assertions")]

use aether_support::{KitErrorCode, SkillRef};
use serde_json::json;
use tempfile::TempDir;

use super::{ConversationStore, SqliteStore, Turn, TurnRecord, TurnStatus};
use crate::prepare::trim_history;
use aether_chat::{EndStatus, Message, ToolCall};
use aether_images::ImagePart;

fn run_input(input: &super::ReservationInput) -> aether_chat::RunInput {
    aether_chat::RunInput {
        run_id: "test-run".into(),
        purpose: "chat".into(),
        permission_mode: aether_support::PermissionMode::Default,
        connection: aether_support::Connection {
            base_url: "https://example.com/v1".into(),
            protocol: aether_support::Protocol::OpenaiChat,
            api_keys: vec![],
            model_id: Some("m1".into()),
            endpoint_id: Some("e1".into()),
            context_window: None,
            max_output: None,
            supports_tools: None,
            reasoning: None,
            native_web_search: false,
            can_vision: true,
            pricing: None,
            headers: vec![],
            image_base_url: None,
            image_edit_base_url: None,
        },
        system: vec![],
        history: trim_history(&input.history_path, u32::MAX),
        input: input.input.clone(),
        images: input.images.clone(),
        tools: vec![],
        budget: aether_chat::Budget::default(),
        reasoning_effort: aether_support::ReasoningEffort::Default,
        native_web_search: false,
    }
}

type ActionArgs<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
    &'a [ImagePart],
    bool,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

type ReservationArgs<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
    &'a [ImagePart],
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

macro_rules! resolved {
    ($store:expr, $($arg:expr),+ $(,)?) => {
        resolved_impl($store, ($($arg),+))
    };
}

macro_rules! reserve_request {
    ($store:expr, $cid:expr, $scope:expr, $input:expr, $images:expr, $parent:expr, $retry:expr, $translation:expr, $language:expr $(,)?) => {
        reserve_request_impl(
            $store,
            (
                $cid,
                $scope,
                $input,
                $images,
                $parent,
                $retry,
                $translation,
                $language,
            ),
        )
    };
}

fn resolved_impl(
    store: &SqliteStore,
    args: ActionArgs<'_>,
) -> Result<(super::ActionRequest, super::ReservationInput), aether_support::KitError> {
    let (cid, scope, input, images, regenerate, parent, retry, translation, language) = args;
    let conversation = cid.map(|id| store.load(id)).transpose()?;
    super::resolve_action(
        conversation.as_ref(),
        cid.map(str::to_owned),
        scope.map(str::to_owned),
        input.to_owned(),
        images.to_vec(),
        regenerate,
        parent.map(str::to_owned),
        retry.map(str::to_owned),
        translation.map(str::to_owned),
        language.map(str::to_owned),
    )
}

fn reserve_request_impl(
    store: &SqliteStore,
    args: ReservationArgs<'_>,
) -> Result<super::TurnHandle, aether_support::KitError> {
    let (cid, scope, input, images, parent, retry, translation, language) = args;
    let (action, _) = resolved_impl(
        store,
        (
            cid,
            scope,
            input,
            images,
            false,
            parent,
            retry,
            translation,
            language,
        ),
    )?;
    store
        .reserve_action(&action, &mut |input| Ok(run_input(input)))
        .map(|r| r.handle)
}

fn reserve_reply(
    store: &SqliteStore,
    cid: Option<&str>,
    scope: Option<&str>,
    input: &str,
) -> Result<super::TurnHandle, aether_support::KitError> {
    reserve_request!(store, cid, scope, input, &[], None, None, None, None)
}
fn reserve_selected_retry(
    store: &SqliteStore,
    cid: &str,
) -> Result<super::TurnHandle, aether_support::KitError> {
    let (action, _) = resolved_impl(
        store,
        (Some(cid), None, "", &[], true, None, None, None, None),
    )?;
    store
        .reserve_action(&action, &mut |input| Ok(run_input(input)))
        .map(|r| r.handle)
}

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
        cost: Some(aether_support::Cost {
            amount: 0.25,
            currency: aether_support::Currency::Cny,
        }),
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
        cost: None,
        created_at: 0,
        images: vec![],
        generated: vec![],
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
            Message::User(_) | Message::UserImages { .. } | Message::Tool { .. } => vec![],
        })
        .collect();
    let results: Vec<&str> = history
        .iter()
        .filter_map(|message| match message {
            Message::Tool { call_id, .. } => Some(call_id.as_str()),
            Message::User(_) | Message::UserImages { .. } | Message::Assistant { .. } => None,
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
        .map(crate::prepare::history::message_chars)
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
    let first = reserve_reply(
        &store,
        None,
        Some("doc-1"),
        "line one\r\nline two that is long enough to be cut at forty",
    )
    .unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first, completed(tool_turn(1, "").messages))
        .unwrap();
    let second = reserve_reply(&store, Some(&id), None, "again").unwrap();
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
    assert_eq!(one.cost, completed(vec![]).cost, "cost survives reopen");
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
        reserve_reply(&store, Some(&id), None, "x")
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
}

#[test]
fn running_turn_left_by_a_crash_becomes_failed() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let handle = reserve_reply(&store, None, None, "hi").unwrap();
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
    let first = reserve_reply(&store, None, None, "q1").unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first.clone(), completed(tool_turn(1, "").messages))
        .unwrap();
    let second = reserve_reply(&store, Some(&id), None, "q2").unwrap();
    assert_eq!(
        reserve_selected_retry(&store, &id)
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_running")
    );
    store
        .finish_turn(second.clone(), completed(tool_turn(2, "").messages))
        .unwrap();
    let before = store.load(&id).unwrap();
    let again = reserve_selected_retry(&store, &id).unwrap();
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
    let next = reserve_reply(&store, Some(&id), None, "q3").unwrap();
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
fn images_persist_copy_on_retry_replay_and_expire() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let png = ImagePart {
        mime: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    };
    let first = reserve_request!(
        &store,
        None,
        None,
        "look",
        std::slice::from_ref(&png),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first.clone(), completed(answer("a cat")))
        .unwrap();
    let retry = reserve_request!(
        &store,
        Some(&id),
        None,
        "ignored",
        &[],
        None,
        Some(&first.turn_id),
        None,
        None,
    )
    .unwrap();
    store
        .finish_turn(retry, completed(answer("a dog")))
        .unwrap();
    assert_eq!(
        reserve_request!(
            &store,
            Some(&id),
            None,
            "x",
            std::slice::from_ref(&png),
            None,
            Some(&first.turn_id),
            None,
            None
        )
        .unwrap_err()
        .details()
        .reason(),
        Some("action_conflict")
    );
    drop(store);
    let store = SqliteStore::open(dir.path()).unwrap();
    let turns = store.load(&id).unwrap().turns;
    assert!(
        turns.iter().all(|t| t.image_parts() == vec![png.clone()]),
        "{turns:?}"
    );
    assert_eq!(
        trim_history(&turns[..1], u32::MAX)[0],
        Message::UserImages {
            text: "look".into(),
            images: vec![png]
        }
    );
    assert_eq!(store.purge_images(i64::MAX).unwrap(), 2);
    let expired = store.load(&id).unwrap().turns;
    assert!(
        expired
            .iter()
            .all(|t| t.images.len() == 1 && t.images[0].data.is_none())
    );
    assert_eq!(
        trim_history(&expired[..1], u32::MAX)[0],
        Message::User("look".into())
    );
    store.delete(&id).unwrap();
}

#[test]
fn edit_compare_and_swap_preserves_usage_and_invalidates_native_context() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let handle = reserve_reply(&store, None, None, "question").unwrap();
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
fn running_foreign_and_dependent_targets_are_rejected_atomically() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = reserve_reply(&store, None, None, "first").unwrap();
    let id = &first.conversation_id;
    store
        .finish_turn(first.clone(), completed(answer("first reply")))
        .unwrap();
    let foreign = reserve_reply(&store, None, None, "foreign").unwrap();
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
            reserve_request!(
                &store,
                Some(id),
                None,
                "ignored",
                &[],
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
    let candidate = reserve_selected_retry(&store, id).unwrap();
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
        reserve_reply(&store, Some(id), None, "next")
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
    let removed = store.remove_turn(id, &candidate.turn_id).unwrap();
    assert_eq!(removed.turns.len(), 1);
    assert_eq!(removed.turns[0].input, "first");
    let child = reserve_reply(&store, Some(id), None, "child").unwrap();
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
    let reparented = store.remove_turn(id, &first.turn_id).unwrap();
    assert_eq!(reparented.summary.selected_turn_id, Some(child.turn_id));
    assert_eq!(reparented.turns.len(), 1);
    assert_eq!(reparented.turns[0].parent_id, None);
}

fn finished(store: &SqliteStore, conversation: Option<&str>, input: &str) -> String {
    let turn = reserve_reply(store, conversation, None, input).unwrap();
    let id = turn.turn_id.clone();
    store.finish_turn(turn, completed(answer(input))).unwrap();
    id
}

#[test]
fn deleting_selected_only_reply_returns_context_to_previous_turn() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let start = reserve_reply(&store, None, None, "q1").unwrap();
    let (id, first) = (start.conversation_id.clone(), start.turn_id.clone());
    store.finish_turn(start, completed(answer("a1"))).unwrap();
    let second = finished(&store, Some(&id), "q2");
    let c = store.remove_turn(&id, &second).unwrap();
    assert_eq!(c.summary.selected_turn_id, Some(first.clone()));
    assert_eq!(c.turns.len(), 1);
    let c = store.remove_turn(&id, &first).unwrap();
    assert_eq!(c.summary.selected_turn_id, None);
    assert!(c.turns.is_empty());
}

#[test]
fn deleting_selected_candidate_moves_context_to_next_candidate() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = reserve_reply(&store, None, None, "q").unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first.clone(), completed(answer("a")))
        .unwrap();
    let second = reserve_selected_retry(&store, &id).unwrap();
    store
        .finish_turn(second.clone(), completed(answer("b")))
        .unwrap();
    let third = reserve_selected_retry(&store, &id).unwrap();
    store
        .finish_turn(third.clone(), completed(answer("c")))
        .unwrap();
    store.select_turn(&id, &second.turn_id).unwrap();
    let fourth = reserve_selected_retry(&store, &id).unwrap();
    store
        .finish_turn(fourth.clone(), completed(answer("d")))
        .unwrap();
    let c = store.remove_turn(&id, &second.turn_id).unwrap();
    assert_eq!(c.summary.selected_turn_id, Some(third.turn_id.clone()));
    assert_eq!(c.turns[2].supersedes_id, Some(third.turn_id.clone()));
    let c = store.remove_turn(&id, &third.turn_id).unwrap();
    assert_eq!(c.summary.selected_turn_id, Some(fourth.turn_id));
    assert_eq!(c.turns.len(), 2);
    assert_eq!(c.turns[1].supersedes_id, None);
    assert_eq!(c.turns[1].question_id, first.question_id);
}

#[test]
fn deleting_reply_reparents_later_turns_and_drops_its_translations() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = reserve_reply(&store, None, None, "q1").unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(first.clone(), completed(answer("a1")))
        .unwrap();
    let second = finished(&store, Some(&id), "q2");
    let translation = reserve_request!(
        &store,
        Some(&id),
        None,
        "ignored",
        &[],
        None,
        None,
        Some(&second),
        Some("French"),
    )
    .unwrap();
    store
        .finish_turn(translation, completed(answer("t2")))
        .unwrap();
    store.select_turn(&id, &second).unwrap();
    let third = finished(&store, Some(&id), "q3");
    let c = store.remove_turn(&id, &second).unwrap();
    assert_eq!(c.summary.selected_turn_id, Some(third.clone()));
    assert_eq!(c.turns.len(), 2);
    let path = super::history_path(&c.turns, Some(&third)).unwrap();
    assert_eq!(
        path.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        [first.turn_id.as_str(), third.as_str()]
    );
}

#[test]
fn terminal_failed_cancelled_replies_support_edit_retry_and_translation() {
    for status in [EndStatus::Failed, EndStatus::Cancelled] {
        let dir = TempDir::new().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let source = reserve_reply(&store, None, Some("scope"), "original input").unwrap();
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
        let retry = reserve_request!(
            &store,
            Some(id),
            None,
            "ignored",
            &[],
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
        let translation = reserve_request!(
            &store,
            Some(id),
            None,
            "ignored",
            &[],
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
        let retry_translation = reserve_selected_retry(&store, id).unwrap();
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
    let source = reserve_reply(&store, None, Some("note:scope"), "first question").unwrap();
    let id = &source.conversation_id;
    store
        .finish_turn(source.clone(), completed(tool_turn(1, "").messages))
        .unwrap();
    let candidate = reserve_selected_retry(&store, id).unwrap();
    store
        .finish_turn(candidate.clone(), completed(answer("candidate")))
        .unwrap();
    store.select_turn(id, &candidate.turn_id).unwrap();
    let child = reserve_reply(&store, Some(id), None, "child input").unwrap();
    store
        .finish_turn(child.clone(), completed(tool_turn(2, "").messages))
        .unwrap();
    let tail = reserve_reply(&store, Some(id), None, "not copied").unwrap();
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
        assert_eq!(copied.cost, source.cost);
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
        let next = reserve_reply(&reopened, Some("old"), None, "next").unwrap();
        assert_eq!(next.parent_id.as_deref(), Some("old3"));
    }
}

#[test]
fn reservation_validation_rejection_leaves_no_conversation_or_metadata() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let (action, _) = resolved!(
        &store,
        None,
        Some("scope"),
        "rejected",
        &[],
        false,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let error = store
        .reserve_action(&action, &mut |_| {
            Err(aether_support::KitError::bad_request(
                "images",
                "vision_unsupported",
            ))
        })
        .unwrap_err();
    assert_eq!(error.code(), KitErrorCode::BadRequest);
    assert!(store.list(None, 200).unwrap().is_empty());
    let first = reserve_reply(&store, None, None, "accepted").unwrap();
    let cid = first.conversation_id.clone();
    store
        .finish_turn(first, completed(answer("answer")))
        .unwrap();
    let before = store.load(&cid).unwrap();
    let (action, _) = resolved!(
        &store,
        Some(&cid),
        None,
        "rejected",
        &[],
        false,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    store
        .reserve_action(&action, &mut |_| {
            Err(aether_support::KitError::conflict("closing"))
        })
        .unwrap_err();
    assert_eq!(store.load(&cid).unwrap(), before);
}

#[test]
fn changed_source_or_image_availability_rejects_before_callback() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let image = ImagePart {
        mime: "image/png".into(),
        data: "aGVsbG8=".into(),
    };
    let source = reserve_request!(
        &store,
        None,
        None,
        "image question",
        &[image],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let cid = &source.conversation_id;
    store
        .finish_turn(source.clone(), completed(answer("answer")))
        .unwrap();
    let (retry, _) = resolved!(
        &store,
        Some(cid),
        None,
        "ignored",
        &[],
        false,
        None,
        Some(&source.turn_id),
        None,
        None,
    )
    .unwrap();
    store.purge_images(i64::MAX).unwrap();
    let before = store.load(cid).unwrap();
    let mut called = false;
    let error = store
        .reserve_action(&retry, &mut |input| {
            called = true;
            Ok(run_input(input))
        })
        .unwrap_err();
    assert_eq!(error.details().reason(), Some("preparation_changed"));
    assert!(!called);
    assert_eq!(store.load(cid).unwrap(), before);
    let (translation, _) = resolved!(
        &store,
        Some(cid),
        None,
        "",
        &[],
        false,
        None,
        None,
        Some(&source.turn_id),
        Some("French"),
    )
    .unwrap();
    store
        .edit_turn(cid, &source.turn_id, "changed answer", "answer")
        .unwrap();
    let before = store.load(cid).unwrap();
    assert_eq!(
        store
            .reserve_action(&translation, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("preparation_changed")
    );
    assert_eq!(store.load(cid).unwrap(), before);
}

#[test]
fn implicit_selection_is_guarded_but_explicit_source_and_rename_are_not() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let source = reserve_reply(&store, None, None, "question").unwrap();
    let cid = &source.conversation_id;
    store
        .finish_turn(source.clone(), completed(answer("first")))
        .unwrap();
    let alternate = reserve_selected_retry(&store, cid).unwrap();
    store
        .finish_turn(alternate.clone(), completed(answer("alternate")))
        .unwrap();
    let (implicit, _) = resolved!(
        &store,
        Some(cid),
        None,
        "next",
        &[],
        false,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let (explicit, _) = resolved!(
        &store,
        Some(cid),
        None,
        "ignored",
        &[],
        false,
        None,
        Some(&source.turn_id),
        None,
        None,
    )
    .unwrap();
    store.select_turn(cid, &alternate.turn_id).unwrap();
    assert_eq!(
        store
            .reserve_action(&implicit, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("preparation_changed")
    );
    store.rename(cid, "unrelated title").unwrap();
    let reservation = store
        .reserve_action(&explicit, &mut |input| {
            let mut run = run_input(input);
            run.run_id = "callback-result".into();
            Ok(run)
        })
        .unwrap();
    assert_eq!(
        reservation.handle.supersedes_id,
        Some(source.turn_id.clone())
    );
    assert_eq!(reservation.input.input, "question");
    assert_eq!(reservation.run_input.run_id, "callback-result");
    assert_eq!(store.load(cid).unwrap().summary.title, "unrelated title");
}

#[test]
fn latest_completed_guard_and_running_reservation_are_atomic_across_connections() {
    let dir = TempDir::new().unwrap();
    let first_store = SqliteStore::open(dir.path()).unwrap();
    let second_store = SqliteStore::open(dir.path()).unwrap();
    let first = reserve_reply(&first_store, None, None, "first").unwrap();
    let cid = &first.conversation_id;
    first_store
        .finish_turn(first.clone(), completed(answer("first")))
        .unwrap();
    first_store
        .lock()
        .unwrap()
        .execute(
            "UPDATE conversations SET selected_turn_id=NULL WHERE id=?1",
            [cid],
        )
        .unwrap();
    let (action, _) = resolved!(
        &first_store,
        Some(cid),
        None,
        "next",
        &[],
        false,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let other = reserve_reply(&second_store, Some(cid), None, "other").unwrap();
    assert_eq!(
        first_store
            .reserve_action(&action, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("turn_running")
    );
    second_store
        .finish_turn(other, completed(answer("other")))
        .unwrap();
    second_store
        .lock()
        .unwrap()
        .execute(
            "UPDATE conversations SET selected_turn_id=NULL WHERE id=?1",
            [cid],
        )
        .unwrap();
    assert_eq!(
        first_store
            .reserve_action(&action, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("preparation_changed")
    );
}

#[test]
fn generated_images_and_native_history_content_are_fingerprinted() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = reserve_reply(&store, None, None, "question").unwrap();
    let cid = &first.conversation_id;
    store
        .finish_turn(first.clone(), completed(tool_turn(1, "").messages))
        .unwrap();
    let (action, _) = resolved!(
        &store,
        Some(cid),
        None,
        "next",
        &[],
        false,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    store
        .add_generated(
            &first.turn_id,
            &[ImagePart {
                mime: "image/png".into(),
                data: "YQ==".into(),
            }],
        )
        .unwrap();
    assert_eq!(
        store
            .reserve_action(&action, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("preparation_changed")
    );
    let (action, _) = resolved!(
        &store,
        Some(cid),
        None,
        "next",
        &[],
        false,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    store
        .edit_turn(cid, &first.turn_id, "edited", "done")
        .unwrap();
    assert_eq!(
        store
            .reserve_action(&action, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("preparation_changed")
    );
}

#[test]
fn missing_prepared_source_and_forged_or_finished_handles_cannot_write() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let source = reserve_reply(&store, None, None, "first").unwrap();
    let mut forged = source.clone();
    forged.input = "forged".into();
    assert_eq!(
        store
            .finish_turn(forged, completed(answer("wrong")))
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    store
        .finish_turn(source.clone(), completed(answer("right")))
        .unwrap();
    assert_eq!(
        store
            .finish_turn(source.clone(), completed(answer("twice")))
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
    let (action, _) = resolved!(
        &store,
        Some(&source.conversation_id),
        None,
        "",
        &[],
        false,
        None,
        Some(&source.turn_id),
        None,
        None,
    )
    .unwrap();
    store
        .remove_turn(&source.conversation_id, &source.turn_id)
        .unwrap();
    assert_eq!(
        store
            .reserve_action(&action, &mut |input| Ok(run_input(input)))
            .unwrap_err()
            .details()
            .reason(),
        Some("preparation_changed")
    );
    assert!(
        store
            .load(&source.conversation_id)
            .unwrap()
            .turns
            .is_empty()
    );
}

#[test]
fn retry_preserves_expired_image_rows_and_original_ttl_and_fork_copies_generated() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let source = reserve_request!(
        &store,
        None,
        None,
        "image",
        &[ImagePart {
            mime: "image/png".into(),
            data: "YQ==".into(),
        }],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    store
        .add_generated(
            &source.turn_id,
            &[ImagePart {
                mime: "image/png".into(),
                data: "Yg==".into(),
            }],
        )
        .unwrap();
    store
        .finish_turn(source.clone(), completed(answer("answer")))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE turn_images SET created_at=1 WHERE turn_id=?1 AND kind='input'",
            [&source.turn_id],
        )
        .unwrap();
    store.purge_images(2).unwrap();
    let retry = reserve_selected_retry(&store, &source.conversation_id).unwrap();
    let (data, created): (Option<String>, i64) = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT data,created_at FROM turn_images WHERE turn_id=?1",
            [&retry.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((data, created), (None, 1));
    store
        .finish_turn(retry, completed(answer("retry")))
        .unwrap();
    let fork = store
        .fork(&source.conversation_id, &source.turn_id)
        .unwrap();
    assert_eq!(
        fork.turns[0].images,
        store.load(&source.conversation_id).unwrap().turns[0].images
    );
    assert_eq!(
        fork.turns[0].generated,
        store.load(&source.conversation_id).unwrap().turns[0].generated
    );
    assert_eq!(
        store.add_generated("missing", &[]).unwrap_err().code(),
        KitErrorCode::NotFound
    );
}

#[test]
fn branched_schemas_v3_to_v6_preserve_messages_images_and_cost() {
    for version in 3..=6 {
        let dir = TempDir::new().unwrap();
        let old =
            aether_support::db::open(&dir.path().join("chat.db"), &super::MIGRATIONS[..version])
                .unwrap();
        old.execute("INSERT INTO conversations (id,title,scope,created_at,updated_at) VALUES ('old','title','scope',1,2)", []).unwrap();
        let messages = tool_turn(1, "").messages;
        old.execute("INSERT INTO turns (id,conversation_id,seq,status,input,messages,question_id,kind,created_at) VALUES ('turn','old',1,'completed','question',?1,'turn','reply',1)", [serde_json::to_string(&messages).unwrap()]).unwrap();
        old.execute(
            "UPDATE conversations SET selected_turn_id='turn' WHERE id='old'",
            [],
        )
        .unwrap();
        if version >= 4 {
            old.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at) VALUES ('turn',0,'image/png','YQ==',1)", []).unwrap();
        }
        if version >= 5 {
            old.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at,kind) VALUES ('turn',1,'image/png',NULL,1,'generated')", []).unwrap();
        }
        if version >= 6 {
            old.execute(
                "UPDATE turns SET cost=1.25,cost_currency='CNY' WHERE id='turn'",
                [],
            )
            .unwrap();
        }
        drop(old);
        let store = SqliteStore::open(dir.path()).unwrap();
        let conversation = store.load("old").unwrap();
        assert_eq!(
            conversation.summary.selected_turn_id.as_deref(),
            Some("turn")
        );
        assert_eq!(conversation.turns[0].messages, messages);
        assert_eq!(
            conversation.turns[0].images.len(),
            usize::from(version >= 4)
        );
        assert_eq!(
            conversation.turns[0].generated.len(),
            usize::from(version >= 5)
        );
        if version >= 6 {
            assert_eq!(
                conversation.turns[0].cost,
                Some(aether_support::Cost {
                    amount: 1.25,
                    currency: aether_support::Currency::Cny
                })
            );
        }
        drop(store);
        assert_eq!(
            SqliteStore::open(dir.path()).unwrap().load("old").unwrap(),
            conversation
        );
    }
}

#[test]
fn retry_resolves_source_model_and_ignores_unconsumed_other_candidate() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = reserve_reply(&store, None, None, "question").unwrap();
    let cid = &first.conversation_id;
    let mut record = completed(answer("answer"));
    record.endpoint_id = Some("source-endpoint".into());
    store.finish_turn(first.clone(), record).unwrap();
    let other = reserve_selected_retry(&store, cid).unwrap();
    store
        .finish_turn(other.clone(), completed(answer("other")))
        .unwrap();
    let (action, input) = resolved!(
        &store,
        Some(cid),
        None,
        "ignored",
        &[],
        false,
        None,
        Some(&first.turn_id),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        input.source_model,
        Some(super::SourceModelRef {
            endpoint_id: "source-endpoint".into(),
            model_id: "m1".into()
        })
    );
    store
        .edit_turn(cid, &other.turn_id, "unrelated edit", "other")
        .unwrap();
    let reserved = store
        .reserve_action(&action, &mut |input| Ok(run_input(input)))
        .unwrap();
    assert_eq!(reserved.input, input);
}
