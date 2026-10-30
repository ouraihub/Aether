//! `mcp-server.db`: token hashes and settings (docs/13 §6, §8).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use aether_support::clock::now_millis;

use aether_support::KitError;
use aether_support::db::{self, Migration};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::dto::{IssuedToken, TokenRecord};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "CREATE TABLE tokens (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL,
            created_at INTEGER NOT NULL, last_used_at INTEGER, revoked_at INTEGER
          );
          CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1),
            port INTEGER NOT NULL, autostart INTEGER NOT NULL DEFAULT 0,
            enabled_scopes TEXT NOT NULL DEFAULT '[]');",
}];

/// `last_used_at` is written at most once per this many milliseconds (docs/13 §8).
const LAST_USED_RESOLUTION_MS: i64 = 60_000;

/// What a valid bearer token grants on this request.
#[derive(Debug, Clone)]
pub(crate) struct Grant {
    pub(crate) token_id: String,
    pub(crate) scopes: Vec<String>,
    /// The user's master switch at the time of the request.
    pub(crate) enabled: Vec<String>,
}

impl Grant {
    pub(crate) fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope) && self.enabled.iter().any(|s| s == scope)
    }
}

pub(crate) struct Store {
    db: Mutex<rusqlite::Connection>,
}

impl Store {
    pub(crate) fn open(root: &Path, default_port: u16) -> Result<Self, KitError> {
        let db = db::open(&root.join("mcp-server.db"), MIGRATIONS)?;
        db.execute(
            "INSERT OR IGNORE INTO settings (id, port) VALUES (1, ?1)",
            params![default_port],
        )?;
        Ok(Self { db: Mutex::new(db) })
    }

    pub(crate) fn port(&self) -> Result<u16, KitError> {
        Ok(self
            .lock()?
            .query_row("SELECT port FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })?)
    }

    pub(crate) fn set_port(&self, port: u16) -> Result<(), KitError> {
        self.lock()?
            .execute("UPDATE settings SET port = ?1 WHERE id = 1", params![port])?;
        Ok(())
    }

    pub(crate) fn enabled_scopes(&self) -> Result<Vec<String>, KitError> {
        let json: String = self.lock()?.query_row(
            "SELECT enabled_scopes FROM settings WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        parse_ids(&json)
    }

    pub(crate) fn set_enabled_scopes(&self, ids: &[&str]) -> Result<(), KitError> {
        let json = serde_json::to_string(ids).map_err(KitError::internal)?;
        self.lock()?.execute(
            "UPDATE settings SET enabled_scopes = ?1 WHERE id = 1",
            params![json],
        )?;
        Ok(())
    }

    /// `scopes` must already be validated against the host's declaration.
    pub(crate) fn issue(
        &self,
        prefix: &str,
        name: &str,
        scopes: Vec<String>,
    ) -> Result<IssuedToken, KitError> {
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|error| KitError::internal(std::io::Error::other(error.to_string())))?;
        let token = format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(secret));
        let record = TokenRecord {
            id: aether_support::id::new_id()?,
            name: name.to_owned(),
            scopes,
            created_at: now_millis(),
            last_used_at: None,
            revoked_at: None,
        };
        let scopes_json = serde_json::to_string(&record.scopes).map_err(KitError::internal)?;
        self.lock()?.execute(
            "INSERT INTO tokens (id, name, token_hash, scopes, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![record.id, record.name, hash(&token), scopes_json, record.created_at],
        )?;
        Ok(IssuedToken { record, token })
    }

    /// Idempotent; `not_found` only for an unknown id.
    pub(crate) fn revoke(&self, id: &str) -> Result<(), KitError> {
        let changed = self.lock()?.execute(
            "UPDATE tokens SET revoked_at = COALESCE(revoked_at, ?1) WHERE id = ?2",
            params![now_millis(), id],
        )?;
        if changed == 0 {
            return Err(KitError::not_found("token"));
        }
        Ok(())
    }

    pub(crate) fn tokens(&self) -> Result<Vec<TokenRecord>, KitError> {
        let db = self.lock()?;
        let mut stmt = db.prepare(
            "SELECT id, name, scopes, created_at, last_used_at, revoked_at
             FROM tokens ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                TokenRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    scopes: Vec::new(),
                    created_at: row.get(3)?,
                    last_used_at: row.get(4)?,
                    revoked_at: row.get(5)?,
                },
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (mut record, scopes) = row?;
            record.scopes = parse_ids(&scopes)?;
            Ok(record)
        })
        .collect()
    }

    /// `None` for an unknown or revoked token. Reads the database on every request so a
    /// revocation or master-switch change applies to the very next request.
    pub(crate) fn authenticate(&self, token: &str) -> Result<Option<Grant>, KitError> {
        let db = self.lock()?;
        // Lookup by the SHA-256 of a 256-bit random token: response timing can at most
        // reveal digest prefixes, which do not help find a preimage (docs/13 §8).
        let row = db
            .query_row(
                "SELECT id, scopes FROM tokens WHERE token_hash = ?1 AND revoked_at IS NULL",
                params![hash(token)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((token_id, scopes)) = row else {
            return Ok(None);
        };
        let enabled: String = db.query_row(
            "SELECT enabled_scopes FROM settings WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let now = now_millis();
        db.execute(
            "UPDATE tokens SET last_used_at = ?1
             WHERE id = ?2 AND (last_used_at IS NULL OR last_used_at <= ?3)",
            params![now, token_id, now - LAST_USED_RESOLUTION_MS],
        )?;
        Ok(Some(Grant {
            token_id,
            scopes: parse_ids(&scopes)?,
            enabled: parse_ids(&enabled)?,
        }))
    }

    fn lock(&self) -> Result<MutexGuard<'_, rusqlite::Connection>, KitError> {
        self.db.lock().map_err(|_| {
            KitError::internal(std::io::Error::other("mcp-server store lock poisoned"))
        })
    }
}

/// Lower-case hex SHA-256: the only form a token is ever stored in.
fn hash(token: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(token.as_bytes()) {
        // Writing to a `String` cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn parse_ids(json: &str) -> Result<Vec<String>, KitError> {
    serde_json::from_str(json).map_err(KitError::internal)
}
