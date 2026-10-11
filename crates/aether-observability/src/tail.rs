//! Reading back the newest lines of the active log file (docs/10 §10).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;

const TAIL_BYTES: u64 = 256 * 1024;
const MAX_LIMIT: usize = 2000;

/// One parsed log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogEntry {
    /// Position in the returned list, oldest first.
    #[ts(type = "number")]
    pub seq: u64,
    /// RFC 3339 UTC timestamp as written; empty for unparsable lines.
    pub time: String,
    /// `ERROR` | `WARN` | `INFO` | `DEBUG` | `TRACE`.
    #[ts(type = "'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE'")]
    pub level: String,
    /// Event target (module path, or `frontend` for ingested entries).
    pub target: String,
    /// Message followed by `k=v` fields.
    pub message: String,
}

/// The last `limit` (capped at 2000) lines within the final 256 KiB of `path`.
pub(crate) fn tail_file(path: &Path, limit: usize) -> io::Result<Vec<LogEntry>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // first line is cut mid-way
    }
    let skip = lines.len().saturating_sub(limit.min(MAX_LIMIT));
    Ok(lines[skip..]
        .iter()
        .zip(0..)
        .map(|(line, seq)| parse_line(line, seq))
        .collect())
}

/// Parses `<time> <LEVEL> <target>: <rest>`; anything else becomes an INFO message.
fn parse_line(line: &str, seq: u64) -> LogEntry {
    parse_fixed(line, seq).unwrap_or_else(|| LogEntry {
        seq,
        time: String::new(),
        level: "INFO".to_owned(),
        target: String::new(),
        message: line.to_owned(),
    })
}

fn parse_fixed(line: &str, seq: u64) -> Option<LogEntry> {
    let (time, rest) = line.split_once(' ')?;
    if !time.ends_with('Z') || !time.contains('T') {
        return None;
    }
    let (level, rest) = rest.trim_start().split_once(' ')?;
    if !matches!(level, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE") {
        return None;
    }
    let (target, message) = rest
        .split_once(": ")
        .or_else(|| rest.strip_suffix(':').map(|t| (t, "")))?;
    if target.is_empty() || target.contains(' ') {
        return None;
    }
    Some(LogEntry {
        seq,
        time: time.to_owned(),
        level: level.to_owned(),
        target: target.to_owned(),
        message: message.to_owned(),
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn parses_fixed_format_and_falls_back_for_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(
            &path,
            "2026-10-10T01:02:03.456789Z  WARN aether_chat::run: chat.run.retry attempt=2\n\
             thread 'main' panicked at src/main.rs:1\n\
             2026-10-10T01:02:04.000000Z ERROR frontend: ui.click.failed\n",
        )
        .unwrap();
        let entries = tail_file(&path, 10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            LogEntry {
                seq: 0,
                time: "2026-10-10T01:02:03.456789Z".into(),
                level: "WARN".into(),
                target: "aether_chat::run".into(),
                message: "chat.run.retry attempt=2".into(),
            }
        );
        assert_eq!(entries[1].level, "INFO");
        assert_eq!(entries[1].target, "");
        assert_eq!(
            entries[1].message,
            "thread 'main' panicked at src/main.rs:1"
        );
        assert_eq!((entries[2].seq, entries[2].level.as_str()), (2, "ERROR"));
    }

    #[test]
    fn returns_newest_lines_within_limit_and_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let mut text = String::new();
        for i in 0..10_000 {
            writeln!(
                text,
                "2026-10-10T00:00:00.000000Z  INFO app: line {i:05} {}",
                "x".repeat(40)
            )
            .unwrap();
        }
        std::fs::write(&path, &text).unwrap();
        let three = tail_file(&path, 3).unwrap();
        assert!(three[2].message.starts_with("line 09999"));
        assert!(three[0].message.starts_with("line 09997"));
        let capped = tail_file(&path, 100_000).unwrap();
        assert_eq!(capped.len(), 2000);
        assert!(capped.iter().all(|e| e.target == "app"));
        assert!(
            tail_file(&dir.path().join("missing.log"), 5)
                .unwrap()
                .is_empty()
        );
    }
}
