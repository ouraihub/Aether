//! SQLite open with forward-only migrations and a pre-upgrade backup (docs/10 §7).

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{KitError, KitErrorCode};
impl From<rusqlite::Error> for KitError {
    fn from(error: rusqlite::Error) -> Self {
        Self::internal(error)
    }
}

/// One schema step. `version` is the `user_version` after `sql` ran.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Target version, 1-based and strictly increasing across steps.
    pub version: u32,
    /// Batch executed inside the step's transaction.
    pub sql: &'static str,
}

/// Opens `path` (WAL, `foreign_keys=ON`, `busy_timeout=5000`) and migrates it to the
/// last step. Before upgrading an existing database it is copied to
/// `<path>.bak-v<old version>`; each step commits on its own, so a crash or a failing
/// step leaves earlier steps applied and the next open continues from there.
///
/// # Errors
/// `incompatible_db` when the file has a newer version than `steps` (file untouched);
/// `internal` when SQLite, the backup or a step fails.
///
/// # Panics
/// When `steps` is not numbered 1, 2, 3, … — a programming error.
pub fn open(path: &Path, steps: &[Migration]) -> Result<Connection, KitError> {
    assert!(
        steps.iter().zip(1..).all(|(step, n)| step.version == n),
        "migration versions must be 1, 2, 3, ..."
    );
    let latest = steps.last().map_or(0, |step| step.version);

    let mut conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > latest {
        return Err(KitError::new(
            KitErrorCode::IncompatibleDb,
            "database was written by a newer version",
        ));
    }
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    if current == latest {
        return Ok(conn);
    }
    tracing::info!(from = current, to = latest, "aether.db.migrate");
    if current > 0 {
        backup(&conn, path, current)?;
    }
    for step in steps.iter().filter(|step| step.version > current) {
        // Dropping the transaction on error rolls back this step only.
        let tx = conn.transaction()?;
        tx.execute_batch(step.sql)?;
        tx.pragma_update(None, "user_version", step.version)?;
        tx.commit()?;
    }
    Ok(conn)
}

/// `<path>.bak-v<version>`.
fn backup_path(path: &Path, version: u32) -> PathBuf {
    let mut name = OsString::from(path);
    name.push(format!(".bak-v{version}"));
    name.into()
}

fn backup(conn: &Connection, path: &Path, version: u32) -> Result<(), KitError> {
    let target = backup_path(path, version);
    // VACUUM INTO refuses an existing target (docs/10 §11.3).
    if let Err(error) = fs::remove_file(&target)
        && error.kind() != ErrorKind::NotFound
    {
        return Err(error.into());
    }
    let target = target
        .to_str()
        .ok_or_else(|| KitError::bad_request("path", "not_utf8"))?;
    conn.execute("VACUUM INTO ?1", [target])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;

    const STEP1: Migration = Migration {
        version: 1,
        sql: "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT);",
    };
    const STEP2: Migration = Migration {
        version: 2,
        sql: "CREATE TABLE b (id INTEGER PRIMARY KEY, a_id INTEGER REFERENCES a(id));",
    };
    const CHILD_ENV: &str = "AETHER_DB_SLOW_UPGRADE_CHILD";

    fn version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn has_table(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    #[test]
    fn fresh_db_migrates_to_latest_without_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        let conn = open(&path, &[STEP1, STEP2]).unwrap();
        assert_eq!(version(&conn), 2);
        assert!(has_table(&conn, "a") && has_table(&conn, "b"));
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        let timeout: i64 = conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
        assert!(!backup_path(&path, 0).exists());
    }

    #[test]
    fn reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        open(&path, &[STEP1, STEP2])
            .unwrap()
            .execute("INSERT INTO a (v) VALUES ('kept')", [])
            .unwrap();
        let conn = open(&path, &[STEP1, STEP2]).unwrap();
        assert_eq!(version(&conn), 2);
        let v: String = conn
            .query_row("SELECT v FROM a", [], |row| row.get(0))
            .unwrap();
        assert_eq!(v, "kept");
        assert!(!backup_path(&path, 2).exists());
    }

    #[test]
    fn newer_db_is_rejected_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE z (x); PRAGMA user_version = 3;")
            .unwrap();
        let before = fs::read(&path).unwrap();
        let error = open(&path, &[STEP1, STEP2]).unwrap_err();
        assert_eq!(error.code(), KitErrorCode::IncompatibleDb);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!backup_path(&path, 3).exists());
    }

    #[test]
    fn failing_step_keeps_earlier_steps_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        open(&path, &[STEP1])
            .unwrap()
            .execute("INSERT INTO a (v) VALUES ('v1 row')", [])
            .unwrap();
        // A stale backup with the same name must be replaced, not block VACUUM INTO.
        fs::write(backup_path(&path, 1), b"stale").unwrap();
        let broken = Migration {
            version: 3,
            sql: "CREATE TABLE c (x); INSERT INTO missing VALUES (1);",
        };
        let error = open(&path, &[STEP1, STEP2, broken]).unwrap_err();
        assert_eq!(error.code(), KitErrorCode::Internal);

        let conn = Connection::open(&path).unwrap();
        assert_eq!(version(&conn), 2);
        assert!(has_table(&conn, "b"));
        assert!(!has_table(&conn, "c"));

        let bak = Connection::open(backup_path(&path, 1)).unwrap();
        assert_eq!(version(&bak), 1);
        let v: String = bak
            .query_row("SELECT v FROM a", [], |row| row.get(0))
            .unwrap();
        assert_eq!(v, "v1 row");
        drop(bak);
        drop(conn);
        // Restore the last known-good file, then reopen through the same migration boundary.
        let restored = dir.path().join("restored.db");
        fs::copy(backup_path(&path, 1), &restored).unwrap();
        let recovered = open(&restored, &[STEP1, STEP2]).unwrap();
        assert_eq!(version(&recovered), 2);
        assert!(has_table(&recovered, "b"));
        let value: String = recovered
            .query_row("SELECT v FROM a", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "v1 row");
        recovered
            .execute("INSERT INTO a (v) VALUES ('after restore')", [])
            .unwrap();
        drop(recovered);
        let reopened = open(&restored, &[STEP1, STEP2]).unwrap();
        let count: i64 = reopened
            .query_row("SELECT count(*) FROM a", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    #[should_panic(expected = "migration versions")]
    fn non_sequential_steps_panic() {
        let dir = tempfile::tempdir().unwrap();
        let _ = open(&dir.path().join("x.db"), &[STEP2]);
    }

    /// Runs only as the child of `killed_upgrade_resumes_on_reopen`: step 2 burns CPU
    /// for minutes so the parent can kill the process mid-transaction.
    #[test]
    #[ignore = "child process of killed_upgrade_resumes_on_reopen"]
    fn slow_upgrade_child() {
        let Some(path) = std::env::var_os(CHILD_ENV) else {
            return;
        };
        let slow = Migration {
            version: 2,
            sql: "CREATE TABLE b (x); INSERT INTO b SELECT count(*) FROM (
                    WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c)
                    SELECT n FROM c LIMIT 100000000000);",
        };
        let _ = open(Path::new(&path), &[STEP1, slow]);
    }

    #[test]
    fn killed_upgrade_resumes_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "db::tests::slow_upgrade_child", "--ignored"])
            .env(CHILD_ENV, &path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        // Step 1 committed means the child is now inside step 2's transaction.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(child.try_wait().unwrap().is_none(), "child exited early");
            assert!(Instant::now() < deadline, "child never committed step 1");
            if path.exists()
                && Connection::open(&path)
                    .and_then(|conn| conn.pragma_query_value(None, "user_version", |r| r.get(0)))
                    .is_ok_and(|v: u32| v == 1)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(child.try_wait().unwrap().is_none(), "step 2 finished early");
        child.kill().unwrap();
        child.wait().unwrap();

        // The killed step left nothing behind: still version 1, no table b.
        let raw = Connection::open(&path).unwrap();
        assert_eq!(version(&raw), 1);
        assert!(!has_table(&raw, "b"));
        drop(raw);

        let conn = open(&path, &[STEP1, STEP2]).unwrap();
        assert_eq!(version(&conn), 2);
        assert!(has_table(&conn, "b"));
        assert!(backup_path(&path, 1).exists());
    }
}
