//! UI-facing diagnostic contract, independent of Tauri commands.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Serialized diagnostic level. Native logging configuration uses `tracing::Level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "UPPERCASE")]
#[ts(export, rename = "LogLevel")]
pub enum DiagnosticLevel {
    /// Error.
    Error,
    /// Warning.
    Warn,
    /// Informational.
    Info,
    /// Debug.
    Debug,
    /// Trace.
    Trace,
}

impl From<DiagnosticLevel> for tracing::Level {
    fn from(level: DiagnosticLevel) -> Self {
        match level {
            DiagnosticLevel::Error => Self::ERROR,
            DiagnosticLevel::Warn => Self::WARN,
            DiagnosticLevel::Info => Self::INFO,
            DiagnosticLevel::Debug => Self::DEBUG,
            DiagnosticLevel::Trace => Self::TRACE,
        }
    }
}

impl From<tracing::Level> for DiagnosticLevel {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            _ => Self::Trace,
        }
    }
}

/// Current diagnostic level and availability of the configured log sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogStatus {
    /// Effective level.
    pub level: DiagnosticLevel,
    /// Whether the most recent write, flush or rotation succeeded.
    pub available: bool,
}
