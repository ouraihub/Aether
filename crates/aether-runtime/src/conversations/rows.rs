//! SQLite row decoding and complete conversation loading, without mutations.
use aether_support::KitError;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, Type, ValueRef};
use rusqlite::{OptionalExtension, Row};
use serde::de::DeserializeOwned;

use super::{Conversation, ConversationSummary, Turn, TurnImage, TurnStatus};

pub(super) const SUMMARY_COLUMNS: &str =
    "id, title, scope, created_at, updated_at, selected_turn_id";

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

pub(super) fn summary(row: &Row<'_>) -> rusqlite::Result<ConversationSummary> {
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

pub(super) fn load_conversation(
    db: &rusqlite::Connection,
    id: &str,
) -> Result<Conversation, KitError> {
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

fn json<T: DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}
