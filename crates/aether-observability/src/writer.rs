//! Size/day rotating, synchronous log file writer (docs/10 §9).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;

/// Writes whole lines to `<dir>/<stem>.log`, rotating to `.1..=.keep`. IO failures never
/// propagate: the line is dropped, `available` turns false and the next write retries.
pub(crate) struct RotatingWriter {
    path: PathBuf,
    max_bytes: u64,
    keep: u8,
    daily: bool,
    clock: Clock,
    state: Mutex<State>,
    available: AtomicBool,
}

struct State {
    file: Option<File>,
    size: u64,
    /// UTC day number of the active file's content (log timestamps are UTC too).
    day: u64,
}

impl RotatingWriter {
    pub(crate) fn new(path: PathBuf, max_bytes: u64, keep: u8, daily: bool, clock: Clock) -> Self {
        Self {
            path,
            max_bytes,
            keep,
            daily,
            clock,
            state: Mutex::new(State {
                file: None,
                size: 0,
                day: 0,
            }),
            available: AtomicBool::new(true),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    pub(crate) fn write_line(&self, line: &[u8]) {
        let ok = match self.state.lock() {
            Ok(mut state) => {
                let result = self.write_locked(&mut state, line);
                if result.is_err() {
                    state.file = None;
                }
                result.is_ok()
            }
            Err(_) => false,
        };
        self.available.store(ok, Ordering::Relaxed);
    }

    /// Persists the active file to disk.
    pub(crate) fn flush(&self) -> io::Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log writer lock poisoned"));
        let result = state.and_then(|state| state.file.as_ref().map_or(Ok(()), File::sync_data));
        self.available.store(result.is_ok(), Ordering::Relaxed);
        result
    }

    fn write_locked(&self, state: &mut State, line: &[u8]) -> io::Result<()> {
        let today = day_of((self.clock)());
        if state.file.is_none() {
            self.open(state, today)?;
        }
        let len = line.len() as u64;
        // An empty file is never rotated, so one oversized line cannot loop.
        if state.size > 0
            && (state.size + len > self.max_bytes || (self.daily && state.day != today))
        {
            state.file = None;
            self.rotate()?;
            self.open(state, today)?;
        }
        let file = state
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file not open"))?;
        file.write_all(line)?;
        state.size += len;
        state.day = today;
        Ok(())
    }

    fn open(&self, state: &mut State, today: u64) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let meta = file.metadata()?;
        state.size = meta.len();
        // A file left from an earlier day (e.g. before a restart) still counts as that day.
        state.day = if state.size > 0 {
            meta.modified().map_or(today, day_of)
        } else {
            today
        };
        state.file = Some(file);
        Ok(())
    }

    /// Deletes `.keep`, shifts `.n` → `.n+1`, moves the active file to `.1`.
    fn rotate(&self) -> io::Result<()> {
        if self.keep == 0 {
            return ignore_missing(fs::remove_file(&self.path));
        }
        ignore_missing(fs::remove_file(self.numbered(self.keep)))?;
        for n in (1..self.keep).rev() {
            ignore_missing(fs::rename(self.numbered(n), self.numbered(n + 1)))?;
        }
        ignore_missing(fs::rename(&self.path, self.numbered(1)))
    }

    fn numbered(&self, n: u8) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{n}"));
        name.into()
    }
}

impl Write for &RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_line(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn day_of(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    fn fixed_clock() -> Clock {
        Box::new(SystemTime::now)
    }

    fn rotated(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("app.log."))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn size_rotation_keeps_at_most_keep_files() {
        let dir = tempfile::tempdir().unwrap();
        let writer = RotatingWriter::new(dir.path().join("app.log"), 100, 3, false, fixed_clock());
        for i in 0..40 {
            writer.write_line(format!("line {i:02} ..............................\n").as_bytes());
        }
        assert!(writer.available());
        assert_eq!(rotated(dir.path()), ["app.log.1", "app.log.2", "app.log.3"]);
        assert!(fs::metadata(dir.path().join("app.log")).unwrap().len() <= 100);
        // Newest content is in the active file, the previous chunk in `.1`.
        let active = fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(active.ends_with("line 39 ..............................\n"));
        let one = fs::read_to_string(dir.path().join("app.log.1")).unwrap();
        assert!(one.contains("line 37"));
    }

    #[test]
    fn rotation_blocked_by_directory_marks_unavailable_then_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let writer = RotatingWriter::new(dir.path().join("app.log"), 50, 2, false, fixed_clock());
        fs::create_dir(dir.path().join("app.log.2")).unwrap();
        fs::write(dir.path().join("app.log.2").join("x"), "occupied").unwrap();
        writer.write_line(&[b'a'; 40]);
        assert!(writer.available());
        writer.write_line(&[b'b'; 40]); // needs rotation: removing `.2` fails
        assert!(!writer.available());
        assert_eq!(fs::read(dir.path().join("app.log")).unwrap(), [b'a'; 40]);

        fs::remove_dir_all(dir.path().join("app.log.2")).unwrap();
        writer.write_line(&[b'c'; 40]);
        assert!(writer.available());
        assert_eq!(fs::read(dir.path().join("app.log")).unwrap(), [b'c'; 40]);
        assert_eq!(fs::read(dir.path().join("app.log.1")).unwrap(), [b'a'; 40]);
    }

    #[test]
    fn daily_rotation_follows_injected_clock() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_700_000_000));
        let clock_now = Arc::clone(&now);
        let clock: Clock =
            Box::new(move || UNIX_EPOCH + Duration::from_secs(clock_now.load(Ordering::Relaxed)));
        let writer = RotatingWriter::new(dir.path().join("app.log"), 1 << 20, 3, true, clock);
        writer.write_line(b"day one\n");
        now.fetch_add(60, Ordering::Relaxed);
        writer.write_line(b"same day\n");
        assert!(rotated(dir.path()).is_empty());
        now.fetch_add(86_400, Ordering::Relaxed);
        writer.write_line(b"day two\n");
        assert_eq!(rotated(dir.path()), ["app.log.1"]);
        assert_eq!(
            fs::read_to_string(dir.path().join("app.log.1")).unwrap(),
            "day one\nsame day\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.log")).unwrap(),
            "day two\n"
        );
    }
}
