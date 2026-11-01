//! SQLite schema, transactional conversation mutations and image persistence.
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use aether_chat::{EndStatus, Message};
use aether_images::ImagePart;
use aether_support::KitError;
use aether_support::clock::now_millis;
use aether_support::db::{self, Migration};
use rusqlite::{OptionalExtension, params};

use super::action;
use super::fingerprint::fingerprint;
use super::graph::{ensure_leaf, latest_leaf, target_turn, visible_text};
use super::rows::{SUMMARY_COLUMNS, load_conversation, summary};
use super::{
    ActionRequest, Conversation, ConversationStore, ConversationSummary, ReservationInput,
    ResolvedAction, Turn, TurnHandle, TurnRecord, TurnReservation, TurnStatus, history_path,
};

pub(super) const MIGRATIONS: &[Migration] = &[
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

    pub(super) fn lock(&self) -> Result<MutexGuard<'_, rusqlite::Connection>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("chat store lock poisoned")))
    }
}

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

    fn reserve_action(
        &self,
        action: &ActionRequest,
        validate: &mut dyn FnMut(&ReservationInput) -> Result<aether_chat::RunInput, KitError>,
    ) -> Result<TurnReservation, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let conversation = action
            .conversation_id
            .as_deref()
            .map(|id| {
                let c = load_conversation(&tx, id).map_err(action::preparation_error)?;
                ensure_idle(&tx, id)?;
                Ok::<_, KitError>(c)
            })
            .transpose()?;
        let input = action::reservation_input(action, conversation.as_ref())
            .map_err(action::preparation_error)?;
        if fingerprint(action, conversation.as_ref(), &input)? != action.expected {
            return Err(KitError::conflict("preparation_changed"));
        }
        let run_input = validate(&input)?;
        let now = now_millis();
        let cid = match &action.conversation_id {
            Some(id) => id.clone(),
            None => {
                let id = aether_support::id::new_id()?;
                let title: String = input
                    .input
                    .chars()
                    .filter(|c| !matches!(c, '\r' | '\n'))
                    .take(TITLE_CHARS)
                    .collect();
                tx.execute("INSERT INTO conversations (id,title,scope,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)", params![id,title,action.scope,now])?;
                id
            }
        };
        let turn_id = aether_support::id::new_id()?;
        let source = match &action.action {
            ResolvedAction::Retry { source_id } | ResolvedAction::Translate { source_id, .. } => {
                conversation
                    .as_ref()
                    .and_then(|c| c.turns.iter().find(|t| t.id == *source_id))
            }
            ResolvedAction::Reply { .. } => None,
        };
        let (parent, question, supersedes, kind) = match &action.action {
            ResolvedAction::Reply { parent_id } => {
                (parent_id.clone(), turn_id.clone(), None, "reply".to_owned())
            }
            ResolvedAction::Retry { source_id } => {
                let source = source.ok_or_else(|| KitError::conflict("preparation_changed"))?;
                (
                    source.parent_id.clone(),
                    source.question_id.clone(),
                    Some(source_id.clone()),
                    source.kind.clone(),
                )
            }
            ResolvedAction::Translate { source_id, .. } => (
                Some(source_id.clone()),
                turn_id.clone(),
                None,
                "translation".to_owned(),
            ),
        };
        tx.execute("INSERT INTO turns (id,conversation_id,seq,status,input,parent_id,question_id,supersedes_id,kind,model_id,endpoint_id,created_at) SELECT ?1,?2,COALESCE(MAX(seq),0)+1,'running',?3,?4,?5,?6,?7,?8,?9,?10 FROM turns WHERE conversation_id=?2", params![turn_id,cid,input.input,parent,question,supersedes,kind,source.and_then(|s| s.model_id.as_deref()),source.and_then(|s| s.endpoint_id.as_deref()),now])?;
        if let Some(source) = &supersedes {
            tx.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at,kind) SELECT ?1,idx,mime,data,created_at,kind FROM turn_images WHERE turn_id=?2 AND kind='input'", params![turn_id,source])?;
        } else if matches!(&action.action, ResolvedAction::Reply { .. }) {
            for (idx, image) in input.images.iter().enumerate() {
                let idx = i64::try_from(idx).map_err(KitError::internal)?;
                tx.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at) VALUES (?1,?2,?3,?4,?5)", params![turn_id,idx,image.mime,image.data,now])?;
            }
        }
        tx.execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now, cid],
        )?;
        tx.commit()?;
        let handle = TurnHandle {
            conversation_id: cid,
            turn_id,
            input: input.input.clone(),
            parent_id: parent,
            question_id: question,
            supersedes_id: supersedes,
            kind,
        };
        Ok(TurnReservation {
            handle,
            input,
            run_input,
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
            "UPDATE turns SET status=?1,messages=?2,model_id=?3,skills=?4,error_code=?5,input_tokens=?6,output_tokens=?7,cache_read_tokens=?8,cache_write_tokens=?9,reasoning_tokens=?10,context_usage=?11,reasoning_effort=?12,endpoint_id=?13,cost=?16,cost_currency=?17 WHERE id=?14 AND conversation_id=?15 AND status='running' AND input=?18 AND parent_id IS ?19 AND question_id=?20 AND supersedes_id IS ?21 AND kind=?22",
            params![status,messages,record.model_id,skills,record.error_code,record.input_tokens,record.output_tokens,record.cache_read_tokens,record.cache_write_tokens,record.reasoning_tokens,context_usage,reasoning_effort,record.endpoint_id,turn.turn_id,turn.conversation_id,record.cost.map(|c| c.amount),record.cost.map(|c| c.currency.as_str()),turn.input,turn.parent_id,turn.question_id,turn.supersedes_id,turn.kind],
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
        if title.chars().count() > 200 {
            return Err(KitError::bad_request("title", "too_long"));
        }
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
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM turns WHERE id=?1)",
            [turn_id],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(KitError::not_found("turn"));
        }
        let now = now_millis();
        for image in images {
            tx.execute("INSERT INTO turn_images (turn_id,idx,mime,data,created_at,kind) SELECT ?1,COALESCE(MAX(idx),-1)+1,?2,?3,?4,'generated' FROM turn_images WHERE turn_id=?1", params![turn_id, image.mime, image.data, now])?;
        }
        tx.commit()?;
        Ok(())
    }
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
