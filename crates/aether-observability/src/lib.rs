//! tracing subscriber, rotating log file, runtime level changes, tail and frontend ingest.
//! Hosts call [`init`] once in `main`, before `tauri::Builder`, so logs emitted during other
//! plugins' setup reach the file.
//!
//! Design: `docs/10-详细设计-基础与可观测性.md` part 2.

mod contract;
mod ingest;
mod tail;
mod writer;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aether_support::KitError;
use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
use tracing_subscriber::layer::{Layer, Layered, SubscriberExt};
use tracing_subscriber::registry::{LookupSpan, Registry, Scope};
use tracing_subscriber::reload;

/// Re-exported so hosts do not depend on the private `aether-support` crate (docs/10 §1.1).
pub use aether_support::paths::data_root;
pub use contract::{DiagnosticLevel, LogStatus};
pub use ingest::{IngestBatch, IngestEntry, IngestLevel, IngestReceipt};
pub use tail::LogEntry;
/// Re-exported so hosts can name levels without depending on `tracing` directly.
pub use tracing::Level as LogLevel;

use writer::RotatingWriter;

const LEVEL_FILE: &str = "level.json";

/// Log file and level settings.
#[derive(Debug, Clone, Copy)]
pub struct LogConfig {
    /// File name stem: `app` → `app.log`.
    pub file_stem: &'static str,
    /// Rotate once the active file would exceed this size.
    pub max_bytes: u64,
    /// Rotated files kept (`.1` ..= `.keep`).
    pub keep: u8,
    /// Also rotate when the (UTC) day changes.
    pub daily: bool,
    /// Level used when neither `env_var` nor `level.json` provides one.
    pub default_level: Level,
    /// Environment variable that, when set to a valid level, overrides the persisted level.
    pub env_var: Option<&'static str>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            file_stem: "app",
            max_bytes: 5 * 1024 * 1024,
            keep: 3,
            daily: false,
            default_level: Level::INFO,
            env_var: None,
        }
    }
}

/// Extra layer a host adds to the subscriber (e.g. a separate retrieval log).
pub type ExtraLayer = Box<dyn Layer<Registry> + Send + Sync>;

type Inner = Layered<Option<ExtraLayer>, Registry>;

/// Handle to the installed logger: level control, file state, tail and ingest.
#[derive(Clone)]
pub struct LogHandle {
    filter: reload::Handle<EnvFilter, Inner>,
    writer: Arc<RotatingWriter>,
    level_file: PathBuf,
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Installs the global subscriber writing to `<logs_dir>/<file_stem>.log` and a panic hook.
/// Level priority: `config.env_var` > `logs_dir/level.json` > `config.default_level`.
///
/// # Errors
/// `conflict` when called a second time in the process or when another global subscriber
/// or `log` logger is already installed.
pub fn init(
    logs_dir: &Path,
    config: LogConfig,
    extra: Option<ExtraLayer>,
) -> Result<LogHandle, KitError> {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return Err(KitError::conflict("logging already initialized"));
    }
    let (subscriber, handle) = build(logs_dir, &config, extra);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| KitError::conflict("a global subscriber is already set").with_cause(e))?;
    // Forward `log` records (used by many Tauri plugins); the tracing filter decides.
    tracing_log::LogTracer::init()
        .map_err(|e| KitError::conflict("a log logger is already set").with_cause(e))?;
    install_panic_hook();
    Ok(handle)
}

fn build(
    logs_dir: &Path,
    config: &LogConfig,
    extra: Option<ExtraLayer>,
) -> (impl Subscriber + Send + Sync + 'static, LogHandle) {
    let level_file = logs_dir.join(LEVEL_FILE);
    let level = config
        .env_var
        .and_then(|name| std::env::var(name).ok())
        .and_then(|value| value.trim().parse().ok())
        .or_else(|| read_level(&level_file))
        .unwrap_or(config.default_level);
    let writer = Arc::new(RotatingWriter::new(
        logs_dir.join(format!("{}.log", config.file_stem)),
        config.max_bytes,
        config.keep,
        config.daily,
        Box::new(std::time::SystemTime::now),
    ));
    let (filter, filter_handle) = reload::Layer::new(filter_for(level));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .event_format(LineFormat)
        .with_writer(Arc::clone(&writer));
    let subscriber = Registry::default()
        .with(extra)
        .with(filter)
        .with(file_layer);
    (
        subscriber,
        LogHandle {
            filter: filter_handle,
            writer,
            level_file,
        },
    )
}

impl LogHandle {
    /// Returns the public diagnostic state without exposing the log file path.
    #[must_use]
    pub fn status(&self) -> LogStatus {
        LogStatus {
            level: self.level().into(),
            available: self.available(),
        }
    }

    /// Applies `level` immediately and persists it to `logs/level.json`.
    ///
    /// # Errors
    /// `internal` when the filter cannot be swapped or the level file cannot be written.
    pub fn set_level(&self, level: Level) -> Result<(), KitError> {
        self.filter
            .reload(filter_for(level))
            .map_err(KitError::internal)?;
        let json = serde_json::json!({ "level": level.as_str() }).to_string();
        fs::write(&self.level_file, json).map_err(KitError::internal)
    }

    /// The level currently in effect.
    #[must_use]
    pub fn level(&self) -> Level {
        self.filter
            .with_current(|filter| filter.max_level_hint().and_then(LevelFilter::into_level))
            .ok()
            .flatten()
            .unwrap_or(Level::ERROR)
    }

    /// Active log file.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.writer.path()
    }

    /// Whether the most recent write, flush or rotation succeeded.
    #[must_use]
    pub fn available(&self) -> bool {
        self.writer.available()
    }

    /// Syncs the active file to disk.
    ///
    /// # Errors
    /// The underlying IO error.
    pub fn flush(&self) -> io::Result<()> {
        self.writer.flush()
    }

    /// The newest `limit` (at most 2000) entries from the last 256 KiB of the active file.
    ///
    /// # Errors
    /// The underlying IO error; a missing file yields an empty list.
    pub fn tail(&self, limit: usize) -> io::Result<Vec<LogEntry>> {
        tail::tail_file(self.writer.path(), limit)
    }

    /// Writes valid frontend entries under target `frontend` (docs/10 §10 rules); invalid
    /// or over-limit entries are counted as rejected and summarized in one warning.
    #[must_use]
    pub fn ingest(&self, batch: &IngestBatch) -> IngestReceipt {
        ingest::ingest(batch)
    }
}

fn filter_for(level: Level) -> EnvFilter {
    EnvFilter::new(level.as_str())
}

fn read_level(path: &Path) -> Option<Level> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("level")?.as_str()?.parse().ok()
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Keep one log line per event: `tail` parses line by line.
        tracing::error!(panic = %info.to_string().replace('\n', " "), "app.panic");
        previous(info);
    }));
}

/// `<RFC3339 UTC> <LEVEL> <target>: <message> <k=v ...>` — the format `tail` parses.
struct LineFormat;

impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let normalized = event.normalized_metadata();
        let meta = normalized.as_ref().unwrap_or_else(|| event.metadata());
        SystemTime.format_time(&mut writer)?;
        write!(writer, " {} {}: ", meta.level(), meta.target())?;
        ctx.format_fields(writer.by_ref(), event)?;
        for span in ctx.event_scope().into_iter().flat_map(Scope::from_root) {
            if let Some(fields) = span.extensions().get::<FormattedFields<N>>()
                && !fields.is_empty()
            {
                write!(writer, " {}", fields.fields)?;
            }
        }
        writeln!(writer)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;

    fn batch(json: &str) -> IngestBatch {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn writes_fixed_format_and_changes_level_at_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = build(dir.path(), &LogConfig::default(), None);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("cmd", command = "logs_tail");
            let _guard = span.enter();
            tracing::info!(rounds = 2, "chat.run.finished");
            tracing::debug!("hidden.at.info");
            handle.set_level(Level::DEBUG).unwrap();
            tracing::debug!("shown.at.debug");
            assert_eq!(handle.level(), Level::DEBUG);
        });
        let entries = handle.tail(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, "INFO");
        assert_eq!(entries[0].target, "aether_observability::tests");
        assert_eq!(
            entries[0].message,
            "chat.run.finished rounds=2 command=\"logs_tail\""
        );
        assert!(entries[0].time.ends_with('Z'));
        assert_eq!(entries[1].message, "shown.at.debug command=\"logs_tail\"");
        assert!(handle.available());

        // The persisted level wins over default_level on the next start.
        let (_subscriber, restarted) = build(dir.path(), &LogConfig::default(), None);
        assert_eq!(restarted.level(), Level::DEBUG);
    }

    #[test]
    fn ingest_accepts_valid_entries_and_rejects_rule_breakers() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = build(dir.path(), &LogConfig::default(), None);
        let receipt = tracing::subscriber::with_default(subscriber, || {
            handle.ingest(&batch(
                r#"{"droppedCount":0,"entries":[
                {"level":"warn","event":"ui.dialog.opened","fields":{"count":3,"ok":true,"name":"settings"}},
                {"level":"info","event":"ui.click"},
                {"level":"info","event":"Ui.click"},
                {"level":"info","event":"single"},
                {"level":"info","event":"a.b.c.d.e.f.g"},
                {"level":"info","event":"ui.x","fields":{"Bad-Key":1}},
                {"level":"info","event":"ui.x","fields":{"obj":{"a":1}}},
                {"level":"info","event":"ui.x","fields":{"text":"user typed sk-abcdefghijklmnopqrstuvwxyz"}},
                {"level":"info","event":"ui.x","fields":{"long":"LONG"}}
                ]}"#
                    .replace("LONG", &"x".repeat(201))
                    .as_str(),
            ))
        });
        assert_eq!(
            receipt,
            IngestReceipt {
                accepted: 2,
                rejected: 7
            }
        );
        let entries = handle.tail(10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            (entries[0].level.as_str(), entries[0].target.as_str()),
            ("WARN", "frontend")
        );
        assert_eq!(
            entries[0].message,
            "ui.dialog.opened count=3 name=\"settings\" ok=true"
        );
        assert_eq!(entries[1].message, "ui.click");
        assert_eq!(entries[2].target, "aether_observability::ingest");
        assert_eq!(entries[2].message, "logs.ingest.rejected count=7 dropped=0");
        assert!(entries.iter().all(|e| !e.message.contains("sk-abc")));
    }

    #[test]
    fn ingest_caps_batch_at_fifty_and_rejects_seventeen_fields() {
        let fields: Vec<String> = (0..17).map(|i| format!("\"f{i}\":1")).collect();
        let first = format!(
            r#"{{"level":"info","event":"ui.x","fields":{{{}}}}}"#,
            fields.join(",")
        );
        let rest = vec![r#"{"level":"debug","event":"ui.tick"}"#; 60].join(",");
        let json = format!(r#"{{"droppedCount":4,"entries":[{first},{rest}]}}"#);
        let receipt = handle_free_ingest(&batch(&json));
        // Entry 0 has 17 fields; entries 1..50 are accepted; 50..61 exceed the batch cap.
        assert_eq!(
            receipt,
            IngestReceipt {
                accepted: 49,
                rejected: 12
            }
        );
    }

    fn handle_free_ingest(batch: &IngestBatch) -> IngestReceipt {
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            ingest::ingest(batch)
        })
    }
}
