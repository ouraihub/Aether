//! Frontend log batches written into the Rust log under target `frontend` (docs/10 §10).

use std::fmt::Write as _;

use aether_support::mask::looks_like_secret;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const MAX_ENTRIES: usize = 50;
const MAX_FIELDS: usize = 16;
const MAX_STRING_CHARS: usize = 200;

/// A batch of frontend log entries.
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IngestBatch {
    /// Entries, at most 50 are accepted per batch.
    pub entries: Vec<IngestEntry>,
    /// Entries the frontend dropped before sending (buffer overflow).
    #[ts(type = "number")]
    pub dropped_count: u64,
}

/// One frontend log entry.
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IngestEntry {
    /// Severity.
    pub level: IngestLevel,
    /// Dotted lower-case event name, e.g. `ui.dialog.opened`.
    pub event: String,
    /// Flat fields: bool, number or short non-secret string.
    #[serde(default)]
    #[ts(optional, type = "Record<string, unknown>")]
    pub fields: Option<Map<String, Value>>,
}

/// Frontend log levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum IngestLevel {
    /// `debug`
    Debug,
    /// `info`
    Info,
    /// `warn`
    Warn,
    /// `error`
    Error,
}

/// How many entries of a batch were written or refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IngestReceipt {
    /// Entries written to the log.
    pub accepted: u32,
    /// Entries refused (over the batch limit or failing validation).
    pub rejected: u32,
}

pub(crate) fn ingest(batch: &IngestBatch) -> IngestReceipt {
    let mut receipt = IngestReceipt {
        accepted: 0,
        rejected: 0,
    };
    for (index, entry) in batch.entries.iter().enumerate() {
        match (index < MAX_ENTRIES).then(|| render(entry)).flatten() {
            Some(line) => {
                emit(entry.level, &line);
                receipt.accepted += 1;
            }
            None => receipt.rejected += 1,
        }
    }
    if receipt.rejected > 0 || batch.dropped_count > 0 {
        tracing::warn!(
            count = receipt.rejected,
            dropped = batch.dropped_count,
            "logs.ingest.rejected"
        );
    }
    receipt
}

/// `event k=v ...`, or `None` when the entry breaks a rule.
fn render(entry: &IngestEntry) -> Option<String> {
    if !is_name(&entry.event, 2) {
        return None;
    }
    let mut line = entry.event.clone();
    let fields = entry.fields.iter().flatten();
    if fields.clone().count() > MAX_FIELDS {
        return None;
    }
    for (key, value) in fields {
        let valid = is_name(key, 1)
            && match value {
                Value::Bool(_) | Value::Number(_) => true,
                Value::String(text) => {
                    text.chars().count() <= MAX_STRING_CHARS && !looks_like_secret(text)
                }
                Value::Null | Value::Array(_) | Value::Object(_) => false,
            };
        if !valid {
            return None;
        }
        // Writing to a String cannot fail.
        let _ = write!(line, " {key}={value}");
    }
    Some(line)
}

/// `^[a-z][a-z0-9_]*(\.[a-z0-9_]+){n-1,5}$`: `min_segments`..=6 dot-separated segments.
fn is_name(name: &str, min_segments: usize) -> bool {
    let segments: Vec<&str> = name.split('.').collect();
    (min_segments..=6).contains(&segments.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

fn emit(level: IngestLevel, line: &str) {
    match level {
        IngestLevel::Debug => tracing::debug!(target: "frontend", "{line}"),
        IngestLevel::Info => tracing::info!(target: "frontend", "{line}"),
        IngestLevel::Warn => tracing::warn!(target: "frontend", "{line}"),
        IngestLevel::Error => tracing::error!(target: "frontend", "{line}"),
    }
}
