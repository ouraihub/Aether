//! Optional conversation store: `chat.db`, stored by turn (docs/12 §6).

use std::iter;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_support::db::{self, Migration};
use aether_support::{KitError, SkillRef};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, Type, ValueRef};
use rusqlite::{OptionalExtension, Row, params};
use serde::de::DeserializeOwned;

use crate::types::{EndStatus, ImagePart, Message};

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: "CREATE TABLE conversations (id TEXT PRIMARY KEY, title TEXT NOT NULL, scope TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL); CREATE INDEX conversations_scope ON conversations(scope, updated_at DESC); CREATE TABLE turns (id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE, seq INTEGER NOT NULL, status TEXT NOT NULL CHECK (status IN ('running','completed','failed','cancelled')), input TEXT NOT NULL, messages TEXT NOT NULL DEFAULT '[]', model_id TEXT, skills TEXT NOT NULL DEFAULT '[]', error_code TEXT, input_tokens INTEGER, output_tokens INTEGER, created_at INTEGER NOT NULL, UNIQUE (conversation_id, seq));",
    },
    Migration {
        version: 2,
        sql: "ALTER TABLE turns ADD COLUMN cache_read_tokens INTEGER; ALTER TABLE turns ADD COLUMN cache_write_tokens INTEGER; ALTER TABLE turns ADD COLUMN reasoning_tokens INTEGER; ALTER TABLE turns ADD COLUMN context_usage TEXT; ALTER TABLE turns ADD COLUMN reasoning_effort TEXT; ALTER TABLE turns ADD COLUMN endpoint_id TEXT;",
    },
    Migration {
        version: 3,
        sql: "ALTER TABLE conversations ADD COLUMN selected_turn_id TEXT REFERENCES turns(id) ON DELETE SET NULL; ALTER TABLE turns ADD COLUMN parent_id TEXT REFERENCES turns(id); ALTER TABLE turns ADD COLUMN question_id TEXT NOT NULL DEFAULT ''; ALTER TABLE turns ADD COLUMN supersedes_id TEXT REFERENCES turns(id); ALTER TABLE turns ADD COLUMN kind TEXT NOT NULL DEFAULT 'reply' CHECK (kind IN ('reply','translation')); UPDATE turns SET question_id = id WHERE question_id = ''; UPDATE turns SET parent_id = (SELECT previous.id FROM turns previous WHERE previous.conversation_id = turns.conversation_id AND previous.seq < turns.seq ORDER BY previous.seq DESC LIMIT 1); UPDATE conversations SET selected_turn_id = (SELECT id FROM turns WHERE turns.conversation_id = conversations.id AND status = 'completed' ORDER BY seq DESC LIMIT 1);",
    },
    Migration {
        version: 4,
        sql: "CREATE TABLE turn_images (turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE, idx INTEGER NOT NULL, mime TEXT NOT NULL, data TEXT, created_at INTEGER NOT NULL, PRIMARY KEY(turn_id, idx));",
    },
    Migration {
        version: 5,
        sql: "ALTER TABLE turn_images ADD COLUMN kind TEXT NOT NULL DEFAULT 'input' CHECK (kind IN ('input','generated'));",
    },
    Migration {
        version: 6,
        sql: "ALTER TABLE turns ADD COLUMN cost REAL; ALTER TABLE turns ADD COLUMN cost_currency TEXT;",
    },
];

/// Title = first input without line breaks, cut to this many characters.
const TITLE_CHARS: usize = 40;

/// Where conversations live. Implemented by [`SqliteStore`] and by hosts that keep
/// conversations in their own database.
pub trait ConversationStore: Send + Sync {
    /// Most recently updated first; `scope: None` lists every scope.
    ///
    /// # Errors
    /// `internal` on storage failure.
    fn list(&self, scope: Option<&str>, limit: usize)
    -> Result<Vec<ConversationSummary>, KitError>;
    /// One conversation with all its turns in order.
    ///
    /// # Errors
    /// `not_found{entity:"conversation"}`; `internal`.
    fn load(&self, id: &str) -> Result<Conversation, KitError>;
    /// Reserves a normal reply, retry candidate or translation atomically. `images` belong
    /// to a normal reply only; a retry copies its source's images, a translation has none.
    /// # Errors
    /// Invalid action fields, `bad_request{images, action_conflict}` for images on a
    /// retry/translation, foreign/missing nodes, running conversation or storage failure.
    #[allow(
        clippy::too_many_arguments,
        reason = "allocation-free borrowed action fields mirror the frozen host request"
    )]
    fn begin_action(
        &self,
        conversation_id: Option<&str>,
        scope: Option<&str>,
        input: &str,
        images: &[ImagePart],
        parent_id: Option<&str>,
        retry_id: Option<&str>,
        translation_id: Option<&str>,
        target_language: Option<&str>,
    ) -> Result<TurnHandle, KitError>;
    /// Starts a text-only normal reply from the selected completed context head.
    /// # Errors
    /// Missing conversation, running conversation or storage failure.
    fn begin_turn(
        &self,
        conversation_id: Option<&str>,
        scope: Option<&str>,
        input: &str,
    ) -> Result<TurnHandle, KitError>;
    /// Retries the selected/latest completed reply without destroying its source.
    /// # Errors
    /// Missing turn, running conversation or storage failure.
    fn regenerate(&self, conversation_id: &str) -> Result<TurnHandle, KitError>;
    /// Writes messages and the terminal status atomically.
    /// # Errors
    /// Missing/nonrunning turn or storage failure.
    fn finish_turn(&self, turn: TurnHandle, record: TurnRecord) -> Result<(), KitError>;
    /// Deletes a conversation and all its candidates.
    /// # Errors
    /// Missing conversation or storage failure.
    fn delete(&self, id: &str) -> Result<(), KitError>;
    /// Trims and updates the title.
    /// # Errors
    /// Empty title, missing conversation or storage failure.
    fn rename(&self, id: &str, title: &str) -> Result<ConversationSummary, KitError>;
    /// Replaces a terminal leaf's visible assistant text using compare-and-swap.
    /// # Errors
    /// Invalid text, stale expected text, running/dependent turn or storage failure.
    fn edit_turn(
        &self,
        conversation_id: &str,
        turn_id: &str,
        text: &str,
        expected_text: &str,
    ) -> Result<Conversation, KitError>;
    /// Deletes a turn and its translations, reparenting dependents onto the successor
    /// (next/previous candidate, else parent) and moving a deleted selection to the
    /// successor's newest reply leaf (docs/35 §3.3).
    /// # Errors
    /// Missing turn, running conversation or storage failure.
    fn remove_turn(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError>;
    /// Copies exactly the target's ancestor path into an independent conversation.
    /// # Errors
    /// Missing/invalid history, running conversation or storage failure.
    fn fork(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError>;
    /// Persists a completed context head without cloning the conversation.
    /// # Errors
    /// Missing/noncompleted turn, running conversation or storage failure.
    fn select_turn(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError>;
    /// Clears the bytes of images created before `before_ms`, keeping their rows so they
    /// show as expired; returns how many were cleared.
    /// # Errors
    /// `internal` on storage failure.
    fn purge_images(&self, before_ms: i64) -> Result<usize, KitError>;
    /// Appends images produced by a tool (`generate_image`) during `turn_id`'s run (docs/20 §3).
    /// # Errors
    /// `internal` on storage failure.
    fn add_generated(&self, turn_id: &str, images: &[ImagePart]) -> Result<(), KitError>;
}

/// A row of the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    /// Conversation id.
    pub id: String,
    /// Title.
    pub title: String,
    /// Host-defined scope.
    pub scope: Option<String>,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds.
    pub updated_at: i64,
    /// Completed context head used for subsequent normal input.
    pub selected_turn_id: Option<String>,
}

/// A conversation with its turns, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    /// Metadata.
    pub summary: ConversationSummary,
    /// Turns ordered by `seq`.
    pub turns: Vec<Turn>,
}

/// State of a stored turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    /// Started and not finished (or regenerating).
    Running,
    /// Finished normally; the only status that enters later history.
    Completed,
    /// Failed; `error_code` says why (`interrupted` after a crash).
    Failed,
    /// Stopped by the user.
    Cancelled,
}

/// User input plus every assistant/tool message the run produced for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// Turn id.
    pub id: String,
    /// 1-based position in the conversation.
    pub seq: u32,
    /// State.
    pub status: TurnStatus,
    /// User input.
    pub input: String,
    /// Messages of the run (kept for failed/cancelled turns too, for display).
    pub messages: Vec<Message>,
    /// Model used.
    pub model_id: Option<String>,
    /// Skills used (id + revision, no body).
    pub skills: Vec<SkillRef>,
    /// Error code of a failed turn.
    pub error_code: Option<String>,
    /// Input tokens reported by the upstream.
    pub input_tokens: Option<u32>,
    /// Output tokens reported by the upstream.
    pub output_tokens: Option<u32>,
    /// Tokens read from provider cache.
    pub cache_read_tokens: Option<u32>,
    /// Tokens written to provider cache.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning output tokens.
    pub reasoning_tokens: Option<u32>,
    /// Last request context report or estimate.
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Requested reasoning effort.
    pub reasoning_effort: Option<aether_support::ReasoningEffort>,
    /// Endpoint used for this turn.
    pub endpoint_id: Option<String>,
    /// Cost fixed when the turn finished; never recomputed (docs/41 §4).
    pub cost: Option<aether_support::Cost>,
    /// Conversation owning the graph node.
    pub conversation_id: String,
    /// Previous context head.
    pub parent_id: Option<String>,
    /// Question shared by alternative candidates.
    pub question_id: String,
    /// Source candidate retried by this turn.
    pub supersedes_id: Option<String>,
    /// `reply` or `translation`.
    pub kind: String,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Input images in order; `data: None` once purged.
    pub images: Vec<TurnImage>,
    /// Images a tool generated during this turn; `data: None` once purged. Never replayed.
    pub generated: Vec<TurnImage>,
}

impl Turn {
    /// Images whose bytes are still stored, in order; expired ones are skipped.
    #[must_use]
    pub fn image_parts(&self) -> Vec<ImagePart> {
        self.images
            .iter()
            .filter_map(|image| {
                Some(ImagePart {
                    mime: image.mime.clone(),
                    data: image.data.clone()?,
                })
            })
            .collect()
    }
}

/// A stored input image; `data` (base64) is `None` after [`ConversationStore::purge_images`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnImage {
    /// MIME type.
    pub mime: String,
    /// Base64 bytes.
    pub data: Option<String>,
}

/// A turn being run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnHandle {
    /// Conversation id.
    pub conversation_id: String,
    /// Turn id.
    pub turn_id: String,
    /// Canonical input used for this run.
    pub input: String,
    /// Reserved parent context head.
    pub parent_id: Option<String>,
    /// Reserved candidate group.
    pub question_id: String,
    /// Source candidate for a retry.
    pub supersedes_id: Option<String>,
    /// `reply` or `translation`.
    pub kind: String,
}

/// Result of a turn, written by [`ConversationStore::finish_turn`].
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRecord {
    /// Terminal status.
    pub status: EndStatus,
    /// Messages of the run, including partial text of a failed/cancelled run.
    pub messages: Vec<Message>,
    /// Model used.
    pub model_id: Option<String>,
    /// Skills used.
    pub skills: Vec<SkillRef>,
    /// Error code when `status` is `failed`.
    pub error_code: Option<String>,
    /// Input tokens.
    pub input_tokens: Option<u32>,
    /// Output tokens.
    pub output_tokens: Option<u32>,
    /// Tokens read from provider cache.
    pub cache_read_tokens: Option<u32>,
    /// Tokens written to provider cache.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning output tokens.
    pub reasoning_tokens: Option<u32>,
    /// Last request context report or estimate.
    pub context_usage: Option<aether_support::ContextUsage>,
    /// Requested reasoning effort.
    pub reasoning_effort: Option<aether_support::ReasoningEffort>,
    /// Endpoint used for this turn.
    pub endpoint_id: Option<String>,
    /// Turn cost from the model's pricing.
    pub cost: Option<aether_support::Cost>,
}

/// History for the next request: only `completed` turns, dropping whole turns from the
/// oldest until the estimate (characters / 3) fits `max_tokens`
/// (= `context_window − max_output − system − input`). Whole turns keep every tool call
/// with its result; if the newest completed turn alone does not fit, history is empty.
#[must_use]
pub fn trim_history(turns: &[Turn], max_tokens: u32) -> Vec<Message> {
    let limit = usize::try_from(max_tokens).unwrap_or(usize::MAX);
    let mut kept = Vec::new();
    let mut chars = 0_usize;
    for turn in turns
        .iter()
        .rev()
        .filter(|turn| turn.status == TurnStatus::Completed)
    {
        chars +=
            turn.input.chars().count() + turn.messages.iter().map(message_chars).sum::<usize>();
        if chars.div_ceil(3) > limit {
            break;
        }
        kept.push(turn);
    }
    kept.iter()
        .rev()
        .flat_map(|turn| {
            let images = turn.image_parts();
            let text = turn.input.clone();
            let user = if images.is_empty() {
                Message::User(text)
            } else {
                Message::UserImages { text, images }
            };
            iter::once(user).chain(turn.messages.iter().cloned())
        })
        .collect()
}

/// Resolves one ancestor chain, rejecting missing, cyclic and foreign nodes.
/// # Errors
/// Missing node, cyclic ancestry or a parent from another conversation.
pub fn history_path(turns: &[Turn], head: Option<&str>) -> Result<Vec<Turn>, KitError> {
    let by_id = turns
        .iter()
        .map(|t| (t.id.as_str(), t))
        .collect::<std::collections::HashMap<_, _>>();
    let owner = head
        .and_then(|id| by_id.get(id))
        .map(|t| t.conversation_id.as_str());
    let mut seen = std::collections::HashSet::new();
    let mut path = Vec::new();
    let mut current = head;
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(KitError::conflict("history_cycle"));
        }
        let turn = by_id.get(id).ok_or_else(|| KitError::not_found("turn"))?;
        if Some(turn.conversation_id.as_str()) != owner {
            return Err(KitError::conflict("history_foreign_turn"));
        }
        current = turn.parent_id.as_deref();
        path.push((*turn).clone());
    }
    path.reverse();
    Ok(path)
}

/// Creates the fixed instruction from visible assistant text, excluding tool/reasoning output.
/// # Errors
/// Empty visible reply or invalid target language.
pub fn translation_input(turn: &Turn, language: &str) -> Result<String, KitError> {
    let text = turn
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant { text, .. } if !text.is_empty() => Some(text.as_str()),
            Message::User(_)
            | Message::UserImages { .. }
            | Message::Assistant { .. }
            | Message::Tool { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    translation_input_text(&text, language)
}

fn translation_input_text(text: &str, language: &str) -> Result<String, KitError> {
    if text.trim().is_empty() {
        return Err(KitError::bad_request("translateTurnId", "empty_reply"));
    }
    let language = language.trim();
    if language.is_empty() || language.chars().count() > 80 {
        return Err(KitError::bad_request("targetLanguage", "invalid"));
    }
    Ok(format!(
        "Translate the following reply into {language}. Return only the translation, preserving Markdown formatting.\n\n{text}"
    ))
}
fn message_chars(message: &Message) -> usize {
    match message {
        // ponytail: images are not counted (docs/19 §4, no image budget).
        Message::User(text) | Message::UserImages { text, .. } | Message::Tool { text, .. } => {
            text.chars().count()
        }
        Message::Assistant {
            text,
            tool_calls,
            reasoning,
        } => {
            text.chars().count()
                + reasoning
                    .iter()
                    .map(|block| block.value.to_string().chars().count())
                    .sum::<usize>()
                + tool_calls
                    .iter()
                    .map(|call| call.name.chars().count() + call.args.to_string().chars().count())
                    .sum::<usize>()
        }
    }
}

/// `chat.db` under the data root.
pub struct SqliteStore {
    db: Mutex<rusqlite::Connection>,
}

impl SqliteStore {
    /// Opens (creating) `<root>/chat.db`. Turns left `running` by a crash become
    /// `failed` with `error_code = "interrupted"`.
    ///
    /// # Errors
    /// See [`aether_support::db::open`].
    pub fn open(root: &Path) -> Result<Self, KitError> {
        let db = db::open(&root.join("chat.db"), MIGRATIONS)?;
        let interrupted = db.execute(
            "UPDATE turns SET status = 'failed', error_code = 'interrupted' WHERE status = 'running'",
            [],
        )?;
        if interrupted > 0 {
            tracing::info!(count = interrupted, "chat.store.interrupted");
        }
        Ok(Self { db: Mutex::new(db) })
    }

    fn lock(&self) -> Result<MutexGuard<'_, rusqlite::Connection>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("chat store lock poisoned")))
    }
}

const SUMMARY_COLUMNS: &str = "id, title, scope, created_at, updated_at, selected_turn_id";

impl ConversationStore for SqliteStore {
    fn list(
        &self,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationSummary>, KitError> {
        let db = self.lock()?;
        let mut stmt = db.prepare(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM conversations WHERE ?1 IS NULL OR scope = ?1
             ORDER BY updated_at DESC, id DESC LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![scope, limit], summary)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn load(&self, id: &str) -> Result<Conversation, KitError> {
        let db = self.lock()?;
        load_conversation(&db, id)
    }

    fn begin_action(
        &self,
        conversation_id: Option<&str>,
        scope: Option<&str>,
        input: &str,
        images: &[ImagePart],
        parent_id: Option<&str>,
        retry_id: Option<&str>,
        translation_id: Option<&str>,
        target_language: Option<&str>,
    ) -> Result<TurnHandle, KitError> {
        for (field, id) in [
            ("conversationId", conversation_id),
            ("replyToId", parent_id),
            ("retryTurnId", retry_id),
            ("translateTurnId", translation_id),
        ] {
            if id.is_some_and(|id| id.trim().is_empty() || id.chars().count() > 128) {
                return Err(KitError::bad_request(field, "invalid"));
            }
        }
        if retry_id.is_some() && translation_id.is_some() {
            return Err(KitError::bad_request("action", "multiple"));
        }
        if parent_id.is_some() && (retry_id.is_some() || translation_id.is_some()) {
            return Err(KitError::bad_request("replyToId", "action_conflict"));
        }
        if !images.is_empty() && (retry_id.is_some() || translation_id.is_some()) {
            return Err(KitError::bad_request("images", "action_conflict"));
        }
        if translation_id.is_some() != target_language.is_some() {
            return Err(KitError::bad_request(
                "targetLanguage",
                "translation_required",
            ));
        }
        let now = now_millis();
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let cid = if let Some(id) = conversation_id {
            load_conversation(&tx, id)?;
            ensure_idle(&tx, id)?;
            id.to_owned()
        } else {
            if retry_id.is_some() || translation_id.is_some() || parent_id.is_some() {
                return Err(KitError::bad_request("conversationId", "required"));
            }
            let id = aether_support::id::new_id()?;
            let title: String = input
                .chars()
                .filter(|c| !matches!(c, '\r' | '\n'))
                .take(TITLE_CHARS)
                .collect();
            tx.execute("INSERT INTO conversations (id,title,scope,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)", params![id,title,scope,now])?;
            id
        };
        let conversation = load_conversation(&tx, &cid)?;
        let source = retry_id
            .or(translation_id)
            .map(|id| target_turn(&conversation, id))
            .transpose()?;
        let source_model = source.as_ref().and_then(|s| s.model_id.clone());
        let source_endpoint = source.as_ref().and_then(|s| s.endpoint_id.clone());
        let turn_id = aether_support::id::new_id()?;
        let (parent, question, supersedes, kind, run_input) = if let Some(source) = source.as_ref()
        {
            if source.status == TurnStatus::Running {
                return Err(KitError::conflict("turn_running"));
            }
            if let Some(language) = target_language {
                (
                    Some(source.id.clone()),
                    turn_id.clone(),
                    None,
                    "translation".to_owned(),
                    translation_input_text(&visible_text(source), language)?,
                )
            } else {
                (
                    source.parent_id.clone(),
                    source.question_id.clone(),
                    Some(source.id.clone()),
                    source.kind.clone(),
                    source.input.clone(),
                )
            }
        } else {
            let selected = parent_id
                .map(str::to_owned)
                .or(conversation.summary.selected_turn_id.clone())
                .or_else(|| {
                    conversation
                        .turns
                        .iter()
                        .rev()
                        .find(|t| t.status == TurnStatus::Completed)
                        .map(|t| t.id.clone())
                });
            if let Some(id) = &selected
                && target_turn(&conversation, id)?.status != TurnStatus::Completed
            {
                return Err(KitError::conflict("turn_not_completed"));
            }
            (
                selected,
                turn_id.clone(),
                None,
                "reply".to_owned(),
                input.to_owned(),
            )
        };
        history_path(&conversation.turns, parent.as_deref())?;
        if run_input.chars().count() > 100_000 {
            return Err(KitError::bad_request("input", "too_long"));
        }
        tx.execute("INSERT INTO turns (id,conversation_id,seq,status,input,parent_id,question_id,supersedes_id,kind,model_id,endpoint_id,created_at) SELECT ?1,?2,COALESCE(MAX(seq),0)+1,'running',?3,?4,?5,?6,?7,?8,?9,?10 FROM turns WHERE conversation_id=?2", params![turn_id,&cid,run_input,parent,question,supersedes,kind,source_model,source_endpoint,now])?;
        if let Some(source) = &supersedes {
            tx.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at,kind) SELECT ?1,idx,mime,data,created_at,kind FROM turn_images WHERE turn_id=?2 AND kind='input'", params![turn_id, source])?;
        }
        for (idx, image) in images.iter().enumerate() {
            let idx = i64::try_from(idx).map_err(KitError::internal)?;
            tx.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at) VALUES (?1,?2,?3,?4,?5)", params![turn_id, idx, image.mime, image.data, now])?;
        }
        tx.execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now, &cid],
        )?;
        tx.commit()?;
        Ok(TurnHandle {
            conversation_id: cid,
            turn_id,
            input: run_input,
            parent_id: parent,
            question_id: question,
            supersedes_id: supersedes,
            kind,
        })
    }

    fn begin_turn(
        &self,
        conversation_id: Option<&str>,
        scope: Option<&str>,
        input: &str,
    ) -> Result<TurnHandle, KitError> {
        self.begin_action(conversation_id, scope, input, &[], None, None, None, None)
    }

    fn regenerate(&self, conversation_id: &str) -> Result<TurnHandle, KitError> {
        let c = self.load(conversation_id)?;
        if c.turns.iter().any(|t| t.status == TurnStatus::Running) {
            return Err(KitError::conflict("turn_running"));
        }
        let id = c
            .summary
            .selected_turn_id
            .or_else(|| {
                c.turns
                    .iter()
                    .rev()
                    .find(|t| t.status == TurnStatus::Completed)
                    .map(|t| t.id.clone())
            })
            .ok_or_else(|| KitError::not_found("turn"))?;
        self.begin_action(
            Some(conversation_id),
            None,
            "",
            &[],
            None,
            Some(&id),
            None,
            None,
        )
    }

    fn finish_turn(&self, turn: TurnHandle, record: TurnRecord) -> Result<(), KitError> {
        let messages = serde_json::to_string(&record.messages).map_err(KitError::internal)?;
        let skills = serde_json::to_string(&record.skills).map_err(KitError::internal)?;
        let status = match record.status {
            EndStatus::Completed => "completed",
            EndStatus::Failed => "failed",
            EndStatus::Cancelled => "cancelled",
        };
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let context_usage = record
            .context_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(KitError::internal)?;
        let reasoning_effort = record
            .reasoning_effort
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(KitError::internal)?;
        let changed = tx.execute(
            "UPDATE turns SET status=?1,messages=?2,model_id=?3,skills=?4,error_code=?5,input_tokens=?6,output_tokens=?7,cache_read_tokens=?8,cache_write_tokens=?9,reasoning_tokens=?10,context_usage=?11,reasoning_effort=?12,endpoint_id=?13,cost=?16,cost_currency=?17 WHERE id=?14 AND conversation_id=?15 AND status='running'",
            params![status,messages,record.model_id,skills,record.error_code,record.input_tokens,record.output_tokens,record.cache_read_tokens,record.cache_write_tokens,record.reasoning_tokens,context_usage,reasoning_effort,record.endpoint_id,turn.turn_id,turn.conversation_id,record.cost.map(|c| c.amount),record.cost.map(|c| c.currency.as_str())],
        )?;
        if changed == 0 {
            return Err(KitError::not_found("turn"));
        }
        tx.execute("UPDATE conversations SET updated_at=?1, selected_turn_id=CASE WHEN ?3='completed' AND EXISTS(SELECT 1 FROM turns WHERE id=?4 AND supersedes_id IS NULL) THEN ?4 ELSE selected_turn_id END WHERE id=?2", params![now_millis(),turn.conversation_id,status,turn.turn_id])?;
        tx.commit()?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE conversations SET selected_turn_id=NULL WHERE id=?1",
            [id],
        )?;
        tx.execute(
            "UPDATE turns SET parent_id=NULL, supersedes_id=NULL WHERE conversation_id=?1",
            [id],
        )?;
        if tx.execute("DELETE FROM conversations WHERE id=?1", [id])? == 0 {
            return Err(KitError::not_found("conversation"));
        }
        tx.commit()?;
        Ok(())
    }

    fn rename(&self, id: &str, title: &str) -> Result<ConversationSummary, KitError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(KitError::bad_request("title", "empty"));
        }
        self.lock()?
            .query_row(
                &format!(
                    "UPDATE conversations SET title = ?1 WHERE id = ?2 RETURNING {SUMMARY_COLUMNS}"
                ),
                params![title, id],
                summary,
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("conversation"))
    }

    fn edit_turn(
        &self,
        conversation_id: &str,
        turn_id: &str,
        text: &str,
        expected_text: &str,
    ) -> Result<Conversation, KitError> {
        if text.trim().is_empty() {
            return Err(KitError::bad_request("text", "empty"));
        }
        if text.chars().count() > 24_000 {
            return Err(KitError::bad_request("text", "too_long"));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let c = load_conversation(&tx, conversation_id)?;
        ensure_idle(&tx, conversation_id)?;
        let turn = target_turn(&c, turn_id)?;
        ensure_leaf(&c, turn_id)?;
        if visible_text(turn) != expected_text {
            return Err(KitError::conflict("text_changed"));
        }
        let messages = serde_json::to_string(&vec![Message::Assistant {
            text: text.to_owned(),
            tool_calls: vec![],
            reasoning: vec![],
        }])
        .map_err(KitError::internal)?;
        tx.execute(
            "UPDATE turns SET messages=?1, context_usage=NULL WHERE id=?2 AND conversation_id=?3",
            params![messages, turn_id, conversation_id],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now_millis(), conversation_id],
        )?;
        let result = load_conversation(&tx, conversation_id)?;
        tx.commit()?;
        Ok(result)
    }

    fn remove_turn(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let c = load_conversation(&tx, conversation_id)?;
        ensure_idle(&tx, conversation_id)?;
        let target = target_turn(&c, turn_id)?;
        // Successor (docs/35 §3.3): next sibling reply of the same question, else the
        // previous one, else the parent.
        let siblings: Vec<&Turn> = c
            .turns
            .iter()
            .filter(|t| t.kind == "reply" && t.question_id == target.question_id && t.id != turn_id)
            .collect();
        let sibling = siblings
            .iter()
            .find(|t| t.seq > target.seq)
            .or(siblings.last());
        let successor = sibling
            .map(|t| t.id.clone())
            .or_else(|| target.parent_id.clone());
        let deleted: Vec<&str> = c
            .turns
            .iter()
            .filter(|t| {
                t.id == turn_id
                    || (t.kind == "translation" && t.parent_id.as_deref() == Some(turn_id))
            })
            .map(|t| t.id.as_str())
            .collect();
        let supersedes = sibling.map(|t| t.id.as_str());
        // Reparent before deleting: parent_id/supersedes_id are foreign keys.
        for id in &deleted {
            tx.execute(
                "UPDATE turns SET parent_id=?1 WHERE parent_id=?2 AND conversation_id=?3",
                params![successor, id, conversation_id],
            )?;
            tx.execute(
                "UPDATE turns SET supersedes_id=CASE WHEN id=?1 THEN NULL ELSE ?1 END WHERE supersedes_id=?2 AND conversation_id=?3",
                params![supersedes, id, conversation_id],
            )?;
        }
        for id in &deleted {
            tx.execute(
                "DELETE FROM turns WHERE id=?1 AND conversation_id=?2",
                params![id, conversation_id],
            )?;
        }
        let selected = c.summary.selected_turn_id.as_deref();
        let selected = if selected.is_some_and(|s| deleted.contains(&s)) {
            latest_leaf(&load_conversation(&tx, conversation_id)?, successor)
        } else {
            selected.map(str::to_owned)
        };
        tx.execute(
            "UPDATE conversations SET selected_turn_id=?1, updated_at=?2 WHERE id=?3",
            params![selected, now_millis(), conversation_id],
        )?;
        let result = load_conversation(&tx, conversation_id)?;
        tx.commit()?;
        Ok(result)
    }

    fn select_turn(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let c = load_conversation(&tx, conversation_id)?;
        ensure_idle(&tx, conversation_id)?;
        if target_turn(&c, turn_id)?.status != TurnStatus::Completed {
            return Err(KitError::conflict("turn_not_completed"));
        }
        tx.execute(
            "UPDATE conversations SET selected_turn_id=?1, updated_at=?2 WHERE id=?3",
            params![turn_id, now_millis(), conversation_id],
        )?;
        let result = load_conversation(&tx, conversation_id)?;
        tx.commit()?;
        Ok(result)
    }

    fn fork(&self, conversation_id: &str, turn_id: &str) -> Result<Conversation, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let c = load_conversation(&tx, conversation_id)?;
        ensure_idle(&tx, conversation_id)?;
        let path = history_path(&c.turns, Some(turn_id))?;
        let id = aether_support::id::new_id()?;
        let ids = path
            .iter()
            .map(|t| Ok((t.id.as_str(), aether_support::id::new_id()?)))
            .collect::<Result<std::collections::HashMap<_, _>, KitError>>()?;
        let mut questions = std::collections::HashMap::new();
        for t in &path {
            if !questions.contains_key(t.question_id.as_str()) {
                let question = match ids.get(t.question_id.as_str()) {
                    Some(id) => id.clone(),
                    None => aether_support::id::new_id()?,
                };
                questions.insert(t.question_id.as_str(), question);
            }
        }
        let now = now_millis();
        tx.execute("INSERT INTO conversations (id,title,scope,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)", params![id,c.summary.title,c.summary.scope,now])?;
        for (seq, t) in path.iter().enumerate() {
            let seq = i64::try_from(seq + 1).map_err(KitError::internal)?;
            tx.execute("INSERT INTO turns (id,conversation_id,seq,parent_id,question_id,supersedes_id,kind,status,input,messages,model_id,skills,error_code,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,context_usage,reasoning_effort,endpoint_id,created_at,cost,cost_currency) SELECT ?1,?2,?3,?4,?5,?6,kind,status,input,messages,model_id,skills,error_code,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,context_usage,reasoning_effort,endpoint_id,created_at,cost,cost_currency FROM turns WHERE id=?7 AND conversation_id=?8", params![ids[&t.id.as_str()],id,seq,t.parent_id.as_deref().and_then(|p| ids.get(p)),questions[&t.question_id.as_str()],t.supersedes_id.as_deref().and_then(|s| ids.get(s)),t.id,conversation_id])?;
            tx.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at,kind) SELECT ?1,idx,mime,data,created_at,kind FROM turn_images WHERE turn_id=?2", params![ids[&t.id.as_str()], t.id])?;
        }
        tx.execute(
            "UPDATE conversations SET selected_turn_id=?1 WHERE id=?2",
            params![ids[&turn_id], id],
        )?;
        let result = load_conversation(&tx, &id)?;
        tx.commit()?;
        Ok(result)
    }

    fn purge_images(&self, before_ms: i64) -> Result<usize, KitError> {
        let cleared = self.lock()?.execute(
            "UPDATE turn_images SET data=NULL WHERE created_at<?1 AND data IS NOT NULL",
            [before_ms],
        )?;
        Ok(cleared)
    }

    fn add_generated(&self, turn_id: &str, images: &[ImagePart]) -> Result<(), KitError> {
        let db = self.lock()?;
        let now = now_millis();
        for image in images {
            db.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at,kind) SELECT ?1,COALESCE(MAX(idx),-1)+1,?2,?3,?4,'generated' FROM turn_images WHERE turn_id=?1", params![turn_id, image.mime, image.data, now])?;
        }
        Ok(())
    }
}

impl FromSql for TurnStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(FromSqlError::Other(
                format!("unknown turn status {other}").into(),
            )),
        }
    }
}

fn summary(row: &Row<'_>) -> rusqlite::Result<ConversationSummary> {
    Ok(ConversationSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        scope: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        selected_turn_id: row.get(5)?,
    })
}

fn turn(row: &Row<'_>) -> rusqlite::Result<Turn> {
    Ok(Turn {
        id: row.get(0)?,
        seq: row.get(1)?,
        status: row.get(2)?,
        input: row.get(3)?,
        messages: json(row, 4)?,
        model_id: row.get(5)?,
        skills: json(row, 6)?,
        error_code: row.get(7)?,
        input_tokens: row.get(8)?,
        output_tokens: row.get(9)?,
        cache_read_tokens: row.get(10)?,
        cache_write_tokens: row.get(11)?,
        reasoning_tokens: row.get(12)?,
        context_usage: row
            .get::<_, Option<String>>(13)?
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(13, Type::Text, Box::new(e)))?,
        reasoning_effort: row
            .get::<_, Option<String>>(14)?
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(14, Type::Text, Box::new(e)))?,
        endpoint_id: row.get(15)?,
        conversation_id: row.get(16)?,
        parent_id: row.get(17)?,
        question_id: row.get(18)?,
        supersedes_id: row.get(19)?,
        kind: row.get(20)?,
        created_at: row.get(21)?,
        cost: match (
            row.get::<_, Option<f64>>(22)?,
            row.get::<_, Option<String>>(23)?,
        ) {
            (Some(amount), Some(currency)) => Some(aether_support::Cost {
                amount,
                currency: aether_support::Currency::parse(&currency).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        23,
                        Type::Text,
                        format!("unknown currency {currency}").into(),
                    )
                })?,
            }),
            _ => None,
        },
        images: Vec::new(),
        generated: Vec::new(),
    })
}

fn load_conversation(db: &rusqlite::Connection, id: &str) -> Result<Conversation, KitError> {
    let summary = db
        .query_row(
            &format!("SELECT {SUMMARY_COLUMNS} FROM conversations WHERE id=?1"),
            [id],
            summary,
        )
        .optional()?
        .ok_or_else(|| KitError::not_found("conversation"))?;
    let mut stmt = db.prepare("SELECT id,seq,status,input,messages,model_id,skills,error_code,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,context_usage,reasoning_effort,endpoint_id,conversation_id,parent_id,question_id,supersedes_id,kind,created_at,cost,cost_currency FROM turns WHERE conversation_id=?1 ORDER BY seq")?;
    let mut turns: Vec<Turn> = stmt.query_map([id], turn)?.collect::<Result<_, _>>()?;
    let mut stmt = db.prepare("SELECT i.turn_id,i.mime,i.data,i.kind FROM turn_images i JOIN turns t ON t.id=i.turn_id WHERE t.conversation_id=?1 ORDER BY i.turn_id,i.idx")?;
    let mut rows = stmt.query([id])?;
    while let Some(row) = rows.next()? {
        let turn_id: String = row.get(0)?;
        if let Some(turn) = turns.iter_mut().find(|t| t.id == turn_id) {
            let image = TurnImage {
                mime: row.get(1)?,
                data: row.get(2)?,
            };
            if row.get::<_, String>(3)? == "generated" {
                turn.generated.push(image);
            } else {
                turn.images.push(image);
            }
        }
    }
    Ok(Conversation { summary, turns })
}

fn ensure_idle(db: &rusqlite::Connection, id: &str) -> Result<(), KitError> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM turns WHERE conversation_id=?1 AND status='running')",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(KitError::conflict("turn_running"));
    }
    Ok(())
}

fn target_turn<'a>(c: &'a Conversation, id: &str) -> Result<&'a Turn, KitError> {
    history_path(&c.turns, Some(id))?;
    c.turns
        .iter()
        .find(|t| t.id == id && t.conversation_id == c.summary.id)
        .ok_or_else(|| KitError::not_found("turn"))
}

/// Newest reply leaf below `start` (or below the newest root reply when `None`).
fn latest_leaf(c: &Conversation, start: Option<String>) -> Option<String> {
    let newest_child = |parent: Option<&str>| {
        c.turns
            .iter()
            .filter(|t| t.kind == "reply" && t.parent_id.as_deref() == parent)
            .max_by_key(|t| t.seq)
            .map(|t| t.id.clone())
    };
    let mut current = start.or_else(|| newest_child(None))?;
    while let Some(next) = newest_child(Some(&current)) {
        current = next;
    }
    Some(current)
}

fn ensure_leaf(c: &Conversation, id: &str) -> Result<(), KitError> {
    if c.turns
        .iter()
        .any(|t| t.parent_id.as_deref() == Some(id) || t.supersedes_id.as_deref() == Some(id))
    {
        return Err(KitError::conflict("turn_has_dependents"));
    }
    Ok(())
}

fn visible_text(turn: &Turn) -> String {
    turn.messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant { text, .. } if !text.is_empty() => Some(text.as_str()),
            Message::User(_)
            | Message::UserImages { .. }
            | Message::Assistant { .. }
            | Message::Tool { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn json<T: DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests;
