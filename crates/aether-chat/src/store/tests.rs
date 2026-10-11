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
    }
}

fn tool_turn(seq: u32, input: &str) -> Turn {
    Turn {
        id: format!("t{seq}"),
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
            },
            Message::Tool {
                call_id: format!("c{seq}"),
                text: "result".repeat(10),
            },
            Message::Assistant {
                text: "done".into(),
                tool_calls: vec![],
            },
        ],
        model_id: None,
        skills: vec![],
        error_code: None,
        input_tokens: None,
        output_tokens: None,
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
fn regenerate_reruns_the_last_turn_in_place() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let first = store.begin_turn(None, None, "q1").unwrap();
    let id = first.conversation_id.clone();
    store
        .finish_turn(
            first,
            completed(vec![Message::Assistant {
                text: "a1".into(),
                tool_calls: vec![],
            }]),
        )
        .unwrap();
    let second = store.begin_turn(Some(&id), None, "q2").unwrap();
    assert_eq!(
        store.regenerate(&id).unwrap_err().details().reason(),
        Some("turn_running")
    );
    store
        .finish_turn(second.clone(), completed(tool_turn(2, "").messages))
        .unwrap();

    let again = store.regenerate(&id).unwrap();
    assert_eq!(
        (again.turn_id.as_str(), again.input.as_str()),
        (second.turn_id.as_str(), "q2")
    );
    let turns = store.load(&id).unwrap().turns;
    assert_eq!(turns.len(), 2, "no new turn");
    assert_eq!(turns[1].status, TurnStatus::Running);
    assert!(
        turns[1].messages.is_empty() && turns[1].model_id.is_none() && turns[1].skills.is_empty()
    );
    // The rerun's history is the earlier turns only.
    assert_eq!(
        trim_history(&turns, u32::MAX),
        vec![
            Message::User("q1".into()),
            Message::Assistant {
                text: "a1".into(),
                tool_calls: vec![]
            },
        ]
    );

    let answer = vec![Message::Assistant {
        text: "a2'".into(),
        tool_calls: vec![],
    }];
    store.finish_turn(again, completed(answer.clone())).unwrap();
    let turns = store.load(&id).unwrap().turns;
    assert_eq!(
        (turns[1].status, &turns[1].messages),
        (TurnStatus::Completed, &answer)
    );

    let empty = store.begin_turn(None, None, "x").unwrap().conversation_id;
    store.delete(&empty).unwrap();
    assert_eq!(
        store.regenerate(&empty).unwrap_err().code(),
        KitErrorCode::NotFound
    );
}
