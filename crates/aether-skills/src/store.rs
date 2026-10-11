//! `skills.db`: install after confirmation, hand-written edits, built-in sync and run
//! snapshots (docs/14 §3–§6).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_support::KitError;
use aether_support::db::{self, Migration};
use aether_support::id::new_id;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection as Db, OptionalExtension, Row, Transaction, params};
use sha2::{Digest, Sha256};

use crate::dto::{
    ManualSave, Preview, PreviewAction, PreviewCurrent, SkillDto, SkillMeta, SkillOrigin, SkillRef,
    SkillSnapshot,
};
use crate::frontmatter;
use crate::package::{self, Source};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "
CREATE TABLE skills (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  version TEXT,
  body TEXT NOT NULL,
  license TEXT,
  extra TEXT NOT NULL DEFAULT '{}',
  origin TEXT NOT NULL CHECK (origin IN ('imported','manual','builtin')),
  revision INTEGER NOT NULL,
  digest TEXT NOT NULL,
  position INTEGER NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);",
}];

const META_COLUMNS: &str =
    "id, name, description, version, origin, revision, length(body), updated_at";

/// A skill shipped with the host, synced into `skills.db` on every [`Store::open`].
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSkill {
    /// Unique slug; reserved for this built-in.
    pub name: &'static str,
    /// Description.
    pub description: &'static str,
    /// Version; `None` = unversioned.
    pub version: Option<&'static str>,
    /// Body.
    pub body: &'static str,
}

/// Skills stored in `<root>/skills.db`.
pub struct Store {
    db: Mutex<Db>,
}

/// Fields that define a skill's content; `digest` covers the first four.
struct Content<'a> {
    name: &'a str,
    description: &'a str,
    version: Option<&'a str>,
    body: &'a str,
    license: Option<&'a str>,
    extra: &'a BTreeMap<String, String>,
}

impl Store {
    /// Opens (creating and migrating) `<root>/skills.db` and syncs `builtin` by name:
    /// changed content bumps the revision, built-ins the host no longer ships are
    /// removed, and a name already taken by a user skill is skipped with a `warn`.
    /// Copies made with [`Store::duplicate`] are `manual` and never touched.
    ///
    /// # Errors
    /// `bad_request` for an invalid built-in; `incompatible_db` for a newer schema;
    /// `internal` when SQLite fails.
    pub fn open(root: &Path, builtin: &'static [BuiltinSkill]) -> Result<Self, KitError> {
        let mut db = db::open(&root.join("skills.db"), MIGRATIONS)?;
        sync_builtin(&mut db, builtin)?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Metadata of every skill in display order; bodies are not loaded.
    ///
    /// # Errors
    /// `internal` when SQLite fails.
    pub fn list(&self) -> Result<Vec<SkillMeta>, KitError> {
        list(&*self.lock()?)
    }

    /// One skill with its body.
    ///
    /// # Errors
    /// `not_found{entity:"skill"}`; `internal` when SQLite fails.
    pub fn get(&self, id: &str) -> Result<SkillDto, KitError> {
        let db = self.lock()?;
        db.query_row(
            &format!("SELECT {META_COLUMNS}, body, license, extra FROM skills WHERE id = ?1"),
            [id],
            |row| {
                Ok((
                    meta(row)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| KitError::not_found("skill"))
        .and_then(|(meta, body, license, extra)| {
            Ok(SkillDto {
                meta,
                body,
                license,
                extra: serde_json::from_str(&extra).map_err(KitError::internal)?,
            })
        })
    }

    /// Reads and validates `source` for the user to review. Nothing is written.
    ///
    /// # Errors
    /// The package errors of docs/14 §2; `conflict{reason:"builtin_name"}`; `internal`.
    pub fn preview(&self, source: &Source) -> Result<Preview, KitError> {
        let package = package::read(source)?;
        let skill = &package.skill;
        let current = existing(&*self.lock()?, &skill.name)?
            .map(|(_, version, revision)| PreviewCurrent { version, revision });
        let digest = digest(
            &skill.name,
            &skill.description,
            skill.version.as_deref(),
            &skill.body,
        );
        let action = if current.is_some() {
            PreviewAction::Replace
        } else {
            PreviewAction::Install
        };
        Ok(Preview {
            digest,
            name: package.skill.name,
            description: package.skill.description,
            version: package.skill.version,
            body: package.skill.body,
            license: package.license,
            extra: package.skill.extra,
            action,
            current,
        })
    }

    /// Re-reads `source` and installs it if it still matches the previewed digest
    /// (docs/14 §4). Replacing keeps id and position and bumps the revision.
    ///
    /// # Errors
    /// `conflict{reason:"content_changed"}` when the content differs from the preview
    /// (call [`Store::preview`] again); `conflict{reason:"exists"}` for a taken name without
    /// `replace`; `conflict{reason:"builtin_name"}`; package errors; `internal`.
    pub fn install(
        &self,
        source: &Source,
        confirmed_digest: &str,
        replace: bool,
    ) -> Result<Vec<SkillMeta>, KitError> {
        let package = package::read(source)?;
        let skill = &package.skill;
        let content = Content {
            name: &skill.name,
            description: &skill.description,
            version: skill.version.as_deref(),
            body: &skill.body,
            license: package.license.as_deref(),
            extra: &skill.extra,
        };
        // Passing validation again is not approval: only the previewed bytes are (docs/14 §7.1).
        if content.digest() != confirmed_digest {
            return Err(KitError::conflict("content_changed"));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        match existing(&tx, content.name)? {
            Some(_) if !replace => return Err(KitError::conflict("exists")),
            Some((id, ..)) => update(&tx, &id, &content, SkillOrigin::Imported)?,
            None => insert(&tx, &content, SkillOrigin::Imported)?,
        }
        tx.commit()?;
        list(&db)
    }

    /// Creates (no `id`) or edits a skill. Editing requires `expectedRevision` equal to the
    /// current revision; the revision only grows when the content changes.
    ///
    /// # Errors
    /// `bad_request` for invalid fields, a missing `expectedRevision` or a built-in id;
    /// `not_found{entity:"skill"}`; `conflict{reason:"revision_changed"}`;
    /// `conflict{reason:"exists"|"builtin_name"}` for a taken name; `internal`.
    pub fn save_manual(&self, input: &ManualSave) -> Result<Vec<SkillMeta>, KitError> {
        frontmatter::validate(
            &input.name,
            &input.description,
            input.version.as_deref(),
            &input.body,
        )?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let Some(id) = input.id.as_deref() else {
            ensure_free(&tx, &input.name, None)?;
            let extra = BTreeMap::new();
            insert(&tx, &input.content(&extra, None), SkillOrigin::Manual)?;
            tx.commit()?;
            return list(&db);
        };
        let expected = input
            .expected_revision
            .ok_or_else(|| KitError::bad_request("expectedRevision", "missing"))?;
        let (origin, revision, license, extra) = tx
            .query_row(
                "SELECT origin, revision, license, extra FROM skills WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, SkillOrigin>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("skill"))?;
        if origin == SkillOrigin::Builtin {
            return Err(KitError::bad_request("id", "builtin"));
        }
        if revision != expected {
            return Err(KitError::conflict("revision_changed").with_entity(id));
        }
        ensure_free(&tx, &input.name, Some(id))?;
        let extra = serde_json::from_str(&extra).map_err(KitError::internal)?;
        update(&tx, id, &input.content(&extra, license.as_deref()), origin)?;
        tx.commit()?;
        list(&db)
    }

    /// Copies any skill (typically a built-in) as a `manual` skill named `new_name`.
    ///
    /// # Errors
    /// `bad_request{field:"name"}`; `not_found{entity:"skill"}`;
    /// `conflict{reason:"exists"|"builtin_name"}`; `internal`.
    pub fn duplicate(&self, id: &str, new_name: &str) -> Result<Vec<SkillMeta>, KitError> {
        let skill = self.get(id)?;
        frontmatter::validate(
            new_name,
            &skill.meta.description,
            skill.meta.version.as_deref(),
            &skill.body,
        )?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        ensure_free(&tx, new_name, None)?;
        let content = Content {
            name: new_name,
            description: &skill.meta.description,
            version: skill.meta.version.as_deref(),
            body: &skill.body,
            license: skill.license.as_deref(),
            extra: &skill.extra,
        };
        insert(&tx, &content, SkillOrigin::Manual)?;
        tx.commit()?;
        list(&db)
    }

    /// Sets the display order; `ids` must list every skill exactly once. Revisions are
    /// unchanged.
    ///
    /// # Errors
    /// `bad_request{field:"ids", reason:"mismatch"}`; `internal`.
    pub fn reorder(&self, ids: &[String]) -> Result<Vec<SkillMeta>, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let mut stored: Vec<String> = tx
            .prepare("SELECT id FROM skills")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut wanted = ids.to_vec();
        stored.sort();
        wanted.sort();
        if stored != wanted {
            return Err(KitError::bad_request("ids", "mismatch"));
        }
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE skills SET position = ?1 WHERE id = ?2",
                params![i64::try_from(position).map_err(KitError::internal)?, id],
            )?;
        }
        tx.commit()?;
        list(&db)
    }

    /// Deletes a skill. History that referenced it keeps only `{id, revision}`.
    ///
    /// # Errors
    /// `bad_request{field:"id", reason:"builtin"}`; `not_found{entity:"skill"}`; `internal`.
    pub fn delete(&self, id: &str) -> Result<Vec<SkillMeta>, KitError> {
        let db = self.lock()?;
        let origin: Option<SkillOrigin> = db
            .query_row("SELECT origin FROM skills WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        match origin {
            None => return Err(KitError::not_found("skill")),
            Some(SkillOrigin::Builtin) => return Err(KitError::bad_request("id", "builtin")),
            Some(SkillOrigin::Imported | SkillOrigin::Manual) => {}
        }
        db.execute("DELETE FROM skills WHERE id = ?1", [id])?;
        list(&db)
    }

    /// Pins the selected skills for one run, in the order given (docs/14 §5).
    ///
    /// # Errors
    /// `conflict{reason:"skill_changed", entity:<id>}` when a skill's revision differs or it
    /// was deleted, so the UI refreshes and asks the user to confirm again; `internal`.
    pub fn snapshot(&self, refs: &[SkillRef]) -> Result<Vec<SkillSnapshot>, KitError> {
        let db = self.lock()?;
        let mut stmt =
            db.prepare_cached("SELECT revision, name, body FROM skills WHERE id = ?1")?;
        refs.iter()
            .map(|skill| {
                let row = stmt
                    .query_row([&skill.id], |row| {
                        Ok((row.get::<_, u32>(0)?, row.get(1)?, row.get(2)?))
                    })
                    .optional()?;
                match row {
                    Some((revision, name, body)) if revision == skill.revision => {
                        Ok(SkillSnapshot {
                            id: skill.id.clone(),
                            revision,
                            name,
                            body,
                        })
                    }
                    _ => Err(KitError::conflict("skill_changed").with_entity(&skill.id)),
                }
            })
            .collect()
    }

    fn lock(&self) -> Result<MutexGuard<'_, Db>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("skills store lock poisoned")))
    }
}

impl ManualSave {
    fn content<'a>(
        &'a self,
        extra: &'a BTreeMap<String, String>,
        license: Option<&'a str>,
    ) -> Content<'a> {
        Content {
            name: &self.name,
            description: &self.description,
            version: self.version.as_deref(),
            body: &self.body,
            license,
            extra,
        }
    }
}

impl Content<'_> {
    fn digest(&self) -> String {
        digest(self.name, self.description, self.version, self.body)
    }
}

/// `sha256(name|description|version|body)` with each field length-prefixed, so moving
/// text between fields changes the digest; `None` and `Some("")` differ.
fn digest(name: &str, description: &str, version: Option<&str>, body: &str) -> String {
    let mut hasher = Sha256::new();
    for field in [Some(name), Some(description), version, Some(body)] {
        match field {
            Some(text) => {
                hasher.update([1]);
                hasher.update((text.len() as u64).to_le_bytes());
                hasher.update(text.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sync_builtin(db: &mut Db, builtin: &[BuiltinSkill]) -> Result<(), KitError> {
    let tx = db.transaction()?;
    let extra = BTreeMap::new();
    for skill in builtin {
        frontmatter::validate(skill.name, skill.description, skill.version, skill.body)?;
        let content = Content {
            name: skill.name,
            description: skill.description,
            version: skill.version,
            body: skill.body,
            license: None,
            extra: &extra,
        };
        let row: Option<(String, SkillOrigin)> = tx
            .query_row(
                "SELECT id, origin FROM skills WHERE name = ?1",
                [skill.name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match row {
            None => insert(&tx, &content, SkillOrigin::Builtin)?,
            Some((id, SkillOrigin::Builtin)) => update(&tx, &id, &content, SkillOrigin::Builtin)?,
            Some((_, SkillOrigin::Imported | SkillOrigin::Manual)) => {
                tracing::warn!(name = skill.name, "skills.builtin.name_taken");
            }
        }
    }
    let mut stale: Vec<String> = tx
        .prepare("SELECT name FROM skills WHERE origin = 'builtin'")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    stale.retain(|name| builtin.iter().all(|skill| skill.name != name));
    for name in &stale {
        tx.execute("DELETE FROM skills WHERE name = ?1", [name])?;
    }
    tx.commit()?;
    Ok(())
}

/// The skill currently holding `name`, as `(id, version, revision)`. A built-in name is
/// reserved: `conflict{reason:"builtin_name"}`.
fn existing(db: &Db, name: &str) -> Result<Option<(String, Option<String>, u32)>, KitError> {
    let row: Option<(String, Option<String>, u32, SkillOrigin)> = db
        .query_row(
            "SELECT id, version, revision, origin FROM skills WHERE name = ?1",
            [name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    match row {
        Some((.., SkillOrigin::Builtin)) => Err(KitError::conflict("builtin_name")),
        row => Ok(row.map(|(id, version, revision, _)| (id, version, revision))),
    }
}

/// Fails when `name` belongs to a skill other than `except` (docs/14 §7.5).
fn ensure_free(db: &Db, name: &str, except: Option<&str>) -> Result<(), KitError> {
    match existing(db, name)? {
        Some((id, ..)) if Some(id.as_str()) != except => Err(KitError::conflict("exists")),
        _ => Ok(()),
    }
}

fn insert(
    tx: &Transaction<'_>,
    content: &Content<'_>,
    origin: SkillOrigin,
) -> Result<(), KitError> {
    let now = now_millis();
    tx.execute(
        "INSERT INTO skills (id, name, description, version, body, license, extra, origin,
           revision, digest, position, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9,
           (SELECT COALESCE(MAX(position) + 1, 0) FROM skills), ?10, ?10)",
        params![
            new_id()?,
            content.name,
            content.description,
            content.version,
            content.body,
            content.license,
            serde_json::to_string(content.extra).map_err(KitError::internal)?,
            origin_str(origin),
            content.digest(),
            now,
        ],
    )?;
    Ok(())
}

/// Overwrites the content of `id`; the revision grows only when the digest changes.
fn update(
    tx: &Transaction<'_>,
    id: &str,
    content: &Content<'_>,
    origin: SkillOrigin,
) -> Result<(), KitError> {
    tx.execute(
        "UPDATE skills SET name = ?2, description = ?3, version = ?4, body = ?5, license = ?6,
           extra = ?7, origin = ?8,
           revision = revision + (digest <> ?9), digest = ?9,
           updated_at = CASE WHEN digest <> ?9 THEN ?10 ELSE updated_at END
         WHERE id = ?1",
        params![
            id,
            content.name,
            content.description,
            content.version,
            content.body,
            content.license,
            serde_json::to_string(content.extra).map_err(KitError::internal)?,
            origin_str(origin),
            content.digest(),
            now_millis(),
        ],
    )?;
    Ok(())
}

fn list(db: &Db) -> Result<Vec<SkillMeta>, KitError> {
    let mut stmt = db.prepare_cached(&format!(
        "SELECT {META_COLUMNS} FROM skills ORDER BY position, id"
    ))?;
    let rows = stmt.query_map([], meta)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Maps the leading [`META_COLUMNS`]; `length(body)` counts characters for TEXT.
fn meta(row: &Row<'_>) -> rusqlite::Result<SkillMeta> {
    Ok(SkillMeta {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        version: row.get(3)?,
        origin: row.get(4)?,
        revision: row.get(5)?,
        // Bodies are capped at 64 KiB, so the character count always fits u32.
        body_chars: row.get::<_, u32>(6)? as usize,
        updated_at: row.get(7)?,
    })
}

fn origin_str(origin: SkillOrigin) -> &'static str {
    match origin {
        SkillOrigin::Imported => "imported",
        SkillOrigin::Manual => "manual",
        SkillOrigin::Builtin => "builtin",
    }
}

impl FromSql for SkillOrigin {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "imported" => Ok(Self::Imported),
            "manual" => Ok(Self::Manual),
            "builtin" => Ok(Self::Builtin),
            _ => Err(FromSqlError::InvalidType),
        }
    }
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
