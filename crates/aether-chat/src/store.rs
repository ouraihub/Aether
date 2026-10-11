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

use crate::types::{EndStatus, Message};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "CREATE TABLE conversations (id TEXT PRIMARY KEY, title TEXT NOT NULL, scope TEXT,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
          CREATE INDEX conversations_scope ON conversations(scope, updated_at DESC);
          CREATE TABLE turns (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('running','completed','failed','cancelled')),
            input TEXT NOT NULL, messages TEXT NOT NULL DEFAULT '[]',
            model_id TEXT, skills TEXT NOT NULL DEFAULT '[]',
            error_code TEXT, input_tokens INTEGER, output_tokens INTEGER,
            created_at INTEGER NOT NULL, UNIQUE (conversation_id, seq)
          );",
}];

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
    /// Appends a `running` turn; `conversation_id: None` creates a conversation
    /// (titled from `input`, in `scope`). `scope` is ignored for an existing conversation.
    ///
    /// # Errors
    /// `not_found{entity:"conversation"}`; `internal`.
    fn begin_turn(
        &self,
        conversation_id: Option<&str>,
        scope: Option<&str>,
        input: &str,
    ) -> Result<TurnHandle, KitError>;
    /// Regenerate: clears the last turn's result and sets it `running` again, keeping
    /// its input (no new turn).
    ///
    /// # Errors
    /// `not_found{entity:"turn"}` when the conversation has no turn;
    /// `conflict{reason:"turn_running"}` when the last turn is still running; `internal`.
    fn regenerate(&self, conversation_id: &str) -> Result<TurnHandle, KitError>;
    /// Writes the turn's messages and terminal state in one step.
    ///
    /// # Errors
    /// `not_found{entity:"turn"}` when the turn is gone or no longer running; `internal`.
    fn finish_turn(&self, turn: TurnHandle, record: TurnRecord) -> Result<(), KitError>;
    /// Deletes a conversation and its turns.
    ///
    /// # Errors
    /// `not_found{entity:"conversation"}`; `internal`.
    fn delete(&self, id: &str) -> Result<(), KitError>;
    /// Sets the title (trimmed).
    ///
    /// # Errors
    /// `bad_request{field:"title", reason:"empty"}`; `not_found{entity:"conversation"}`; `internal`.
    fn rename(&self, id: &str, title: &str) -> Result<ConversationSummary, KitError>;
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
    /// Unix milliseconds.
    pub created_at: i64,
}

/// A turn being run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnHandle {
    /// Conversation id.
    pub conversation_id: String,
    /// Turn id.
    pub turn_id: String,
    /// The turn's input; for regenerate this is the stored input to run again.
    pub input: String,
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
            iter::once(Message::User(turn.input.clone())).chain(turn.messages.iter().cloned())
        })
        .collect()
}

fn message_chars(message: &Message) -> usize {
    match message {
        Message::User(text) | Message::Tool { text, .. } => text.chars().count(),
        Message::Assistant { text, tool_calls } => {
            text.chars().count()
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

const SUMMARY_COLUMNS: &str = "id, title, scope, created_at, updated_at";

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
        let summary = db
            .query_row(
                &format!("SELECT {SUMMARY_COLUMNS} FROM conversations WHERE id = ?1"),
                params![id],
                summary,
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("conversation"))?;
        let mut stmt = db.prepare(
            "SELECT id, seq, status, input, messages, model_id, skills, error_code,
                    input_tokens, output_tokens, created_at
             FROM turns WHERE conversation_id = ?1 ORDER BY seq",
        )?;
        let turns = stmt
            .query_map(params![id], turn)?
            .collect::<Result<_, _>>()?;
        Ok(Conversation { summary, turns })
    }

    fn begin_turn(
        &self,
        conversation_id: Option<&str>,
        scope: Option<&str>,
        input: &str,
    ) -> Result<TurnHandle, KitError> {
        let now = now_millis();
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let conversation_id = if let Some(id) = conversation_id {
            let changed = tx.execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
            if changed == 0 {
                return Err(KitError::not_found("conversation"));
            }
            id.to_owned()
        } else {
            let id = aether_support::id::new_id()?;
            let title: String = input
                .chars()
                .filter(|c| !matches!(c, '\r' | '\n'))
                .take(TITLE_CHARS)
                .collect();
            tx.execute(
                "INSERT INTO conversations (id, title, scope, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![id, title, scope, now],
            )?;
            id
        };
        let turn_id = aether_support::id::new_id()?;
        tx.execute(
            "INSERT INTO turns (id, conversation_id, seq, status, input, created_at)
             SELECT ?1, ?2, COALESCE(MAX(seq), 0) + 1, 'running', ?3, ?4 FROM turns WHERE conversation_id = ?2",
            params![turn_id, conversation_id, input, now],
        )?;
        tx.commit()?;
        Ok(TurnHandle {
            conversation_id,
            turn_id,
            input: input.to_owned(),
        })
    }

    fn regenerate(&self, conversation_id: &str) -> Result<TurnHandle, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let (turn_id, status, input): (String, TurnStatus, String) = tx
            .query_row(
                "SELECT id, status, input FROM turns WHERE conversation_id = ?1 ORDER BY seq DESC LIMIT 1",
                params![conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("turn"))?;
        if status == TurnStatus::Running {
            return Err(KitError::conflict("turn_running"));
        }
        tx.execute(
            "UPDATE turns SET status = 'running', messages = '[]', model_id = NULL, skills = '[]',
                    error_code = NULL, input_tokens = NULL, output_tokens = NULL
             WHERE id = ?1",
            params![turn_id],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now_millis(), conversation_id],
        )?;
        tx.commit()?;
        Ok(TurnHandle {
            conversation_id: conversation_id.to_owned(),
            turn_id,
            input,
        })
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
        let changed = tx.execute(
            "UPDATE turns SET status = ?1, messages = ?2, model_id = ?3, skills = ?4, error_code = ?5,
                    input_tokens = ?6, output_tokens = ?7
             WHERE id = ?8 AND conversation_id = ?9 AND status = 'running'",
            params![
                status,
                messages,
                record.model_id,
                skills,
                record.error_code,
                record.input_tokens,
                record.output_tokens,
                turn.turn_id,
                turn.conversation_id
            ],
        )?;
        if changed == 0 {
            return Err(KitError::not_found("turn"));
        }
        tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now_millis(), turn.conversation_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), KitError> {
        if self
            .lock()?
            .execute("DELETE FROM conversations WHERE id = ?1", params![id])?
            == 0
        {
            return Err(KitError::not_found("conversation"));
        }
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
        created_at: row.get(10)?,
    })
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
