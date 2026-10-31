//! `skills.db`: install after confirmation, hand-written edits, built-in sync
//! and run snapshots (docs/14 §3–§6).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_support::KitError;
use aether_support::db::{self, Migration};
use aether_support::id::new_id;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection as Db, OptionalExtension, Row, Transaction, params};

use crate::content::{self, ContentIdentity};
use crate::dto::{
    ManualSave, Preview, PreviewAction, PreviewCurrent, SkillDto, SkillFileKind,
    SkillFileMeta,
    SkillMeta, SkillOrigin, SkillRef, SkillSnapshot,
};
use crate::frontmatter;
use crate::package::{self, PackageFile, Source};

const MIGRATIONS: &[Migration] = &[
    Migration {
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
    },
    Migration {
        version: 2,
        sql: "
CREATE TABLE skill_files (
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('script','reference','asset')),
  size INTEGER NOT NULL,
  sha256 TEXT NOT NULL,
  content BLOB NOT NULL,
  PRIMARY KEY (skill_id, path)
);",
    },
];

/// Leading columns mapped by [`meta`].
const META_COLUMNS: &str =
    "id, name, description, version, origin, revision, length(body), "
    "EXISTS (SELECT 1 FROM skill_files f WHERE f.skill_id = skills.id "
    "AND f.kind = 'script'), "
    "(SELECT COUNT(*) FROM skill_files f WHERE f.skill_id = skills.id)";

/// A skill shipped with the host, synced into `skills.db` on every
/// [`Store::open`].
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

/// Fields that define a skill's content; `extra` is stored but not in the
/// digest.
struct Content<'a> {
    name: &'a str,
    description: &'a str,
    version: Option<&'a str>,
    body: &'a str,
    license: Option<&'a str>,
    extra: &'a BTreeMap<String, String>,
    /// Packaged files; part of the digest.
    files: FileSource<'a>,
}

/// Where a skill's packaged files come from on insert or update.
#[derive(Clone, Copy)]
enum FileSource<'a> {
    /// Exactly these (empty for hand-written and built-in skills).
    Package(&'a [PackageFile]),
    /// The files currently stored for this skill id: kept on an edit, copied
    /// on a duplicate.
}

impl Store {
    /// Opens (creating and migrating) `<root>/skills.db` and syncs `builtin` by
    /// name: changed content bumps the revision, built-ins the host no longer
    /// ships are removed, and a name already taken by a user skill is skipped
    /// with a `warn`. Copies made with [`Store::duplicate`] are `manual` and
    /// never touched.
    ///
    /// # Errors
    /// `bad_request` for an invalid built-in; `incompatible_db` for a newer
    /// schema; `internal` when SQLite fails.
    pub fn open(
        root: &Path,
        builtin: &'static [BuiltinSkill],
    ) -> Result<Self, KitError> {
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

    /// One skill with its body and file list, both read from one snapshot.
    ///
    /// # Errors
    /// `not_found{entity:"skill"}`; `internal` when SQLite fails.
    pub fn get(&self, id: &str) -> Result<SkillDto, KitError> {
        let mut db = self.lock()?;
        let db = db.transaction()?;
        get(&db, id)
    }

    /// The file list of revision `revision` of a skill. The revision check and
    /// the read share one snapshot, so another connection replacing the skill
    /// in between cannot hand out the newer list.
    ///
    /// # Errors
    /// `conflict{reason:"skill_changed", entity:<id>}` when the revision
    /// differs or the skill was deleted; `internal`.
    pub fn files(
        &self,
        id: &str,
        revision: u32,
    ) -> Result<Vec<SkillFileMeta>, KitError> {
        let mut db = self.lock()?;
        let db = db.transaction()?;
        ensure_revision(&db, id, revision)?;
        file_list(&db, id)
    }

    /// Content of one packaged file of revision `revision`. Nothing is executed
    /// or written to disk; running scripts is the host's decision. The revision
    /// check and the read share one snapshot, as in [`Store::files`].
    ///
    /// # Errors
    /// `conflict{reason:"skill_changed", entity:<id>}` when the revision
    /// differs or the skill was deleted; `not_found{entity:"skill_file"}`
    /// for an unknown path; `internal`.
    pub fn file(
        &self,
        id: &str,
        revision: u32,
        path: &str,
    ) -> Result<Vec<u8>, KitError> {
        let mut db = self.lock()?;
        let db = db.transaction()?;
        ensure_revision(&db, id, revision)?;
        db.query_row(
            "SELECT content FROM skill_files WHERE skill_id = ?1 AND path = ?2",
            [id, path],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| KitError::not_found("skill_file"))
    }

    /// Reads and validates `source` for the user to review. Nothing is written.
    ///
    /// # Errors
    /// The package errors of docs/14 §2; `conflict{reason:"builtin_name"}`;
    /// `internal`.
    pub fn preview(&self, source: &Source) -> Result<Preview, KitError> {
        let package = package::read(source)?;
        let db = self.lock()?;
        let current = existing(&db, &package.skill.name)?
            .map(|(_, version, revision)| PreviewCurrent { version, revision });
        let content = Content::package(&package);
        let digest = content::digest(
            content.identity(),
            package
                .files
                .iter()
                .map(|file| (file.path.as_str(), file.sha256.as_str())),
        );
        drop(db);
        let action = if current.is_some() {
            PreviewAction::Replace
        } else {
            PreviewAction::Install
        };
        let files = package
            .files
            .iter()
            .map(|file| SkillFileMeta {
                path: file.path.clone(),
                kind: file.kind,
                size: file.content.len() as u64,
            })
            .collect();
        Ok(Preview {
            digest,
            name: package.skill.name,
            description: package.skill.description,
            version: package.skill.version,
            body: package.skill.body,
            license: package.license,
            extra: package.skill.extra,
            files,
            action,
            current,
        })
    }

    /// Re-reads `source` and installs it if it still matches the previewed
    /// digest (docs/14 §4). Replacing keeps id and position and bumps the
    /// revision.
    ///
    /// # Errors
    /// `conflict{reason:"content_changed"}` when the content differs from the
    /// preview (call [`Store::preview`] again); `conflict{reason:"exists"}` for
    /// a taken name without `replace`; `conflict{reason:"builtin_name"}`;
    /// package errors; `internal`.
    pub fn install(
        &self,
        source: &Source,
        confirmed_digest: &str,
        replace: bool,
    ) -> Result<Vec<SkillMeta>, KitError> {
        let package = package::read(source)?;
        let content = Content::package(&package);
        let mut db = self.lock()?;
        // Passing validation again is not approval: only the previewed bytes
        // are (docs/14 §7.1).
        if content::digest(
            content.identity(),
            package
                .files
                .iter()
                .map(|file| (file.path.as_str(), file.sha256.as_str())),
        ) != confirmed_digest
        {
            return Err(KitError::conflict("content_changed"));
        }
        let tx = db.transaction()?;
        match existing(&tx, content.name)? {
            Some(_) if !replace => return Err(KitError::conflict("exists")),
            Some((id, ..)) => {
                update(&tx, &id, &content, SkillOrigin::Imported)?
            }
            None => insert(&tx, &content, SkillOrigin::Imported)?,
        }
        tx.commit()?;
        list(&db)
    }

    /// Creates (no `id`) or edits a skill. Editing requires `expectedRevision`
    /// equal to the current revision; the revision only grows when the content
    /// changes.
    ///
    /// # Errors
    /// `bad_request` for invalid fields, a missing `expectedRevision` or a
    /// built-in id; `not_found{entity:"skill"}`;
    /// `conflict{reason:"revision_changed"}`;
    /// `conflict{reason:"exists"|"builtin_name"}` for a taken name;
    /// `internal`.
    pub fn save_manual(
        &self,
        input: &ManualSave,
    ) -> Result<Vec<SkillMeta>, KitError> {
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
            let content = input.content(&extra, None, FileSource::Package(&[]));
            insert(&tx, &content, SkillOrigin::Manual)?;
            tx.commit()?;
            return list(&db);
        };
        let expected = input
            .expected_revision
            .ok_or_else(|| {
                KitError::bad_request("expectedRevision", "missing")
            })?;
        let (origin, revision, license, extra) = tx
            .query_row(
                "SELECT origin, revision, license, extra FROM skills "
                "WHERE id = ?1",
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
        let content = input.content(
            &extra,
            license.as_deref(),
            FileSource::Stored(id),
        );
        update(&tx, id, &content, origin)?;
        tx.commit()?;
        list(&db)
    }

    /// Copies any skill (typically a built-in) as a `manual` skill named
    /// `new_name`, packaged files included.
    ///
    /// # Errors
    /// `bad_request{field:"name"}`; `not_found{entity:"skill"}`;
    /// `conflict{reason:"exists"|"builtin_name"}`; `internal`.
    pub fn duplicate(
        &self,
        id: &str,
        new_name: &str,
    ) -> Result<Vec<SkillMeta>, KitError> {
        let mut db = self.lock()?;
        // Reserve the writer before reading so another connection cannot
        // replace the source between its instructions and the packaged-file
        // copy.
        let tx = db.transaction_with_behavior(
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let skill = get(&tx, id)?;
        frontmatter::validate(
            new_name,
            &skill.meta.description,
            skill.meta.version.as_deref(),
            &skill.body,
        )?;
        ensure_free(&tx, new_name, None)?;
        let content = Content {
            name: new_name,
            description: &skill.meta.description,
            version: skill.meta.version.as_deref(),
            body: &skill.body,
            license: skill.license.as_deref(),
            extra: &skill.extra,
            files: FileSource::Stored(id),
        };
        insert(&tx, &content, SkillOrigin::Manual)?;
        tx.commit()?;
        list(&db)
    }

    /// Sets the display order; `ids` must list every skill exactly once.
    /// Revisions are unchanged.
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
                params![
                    i64::try_from(position).map_err(KitError::internal)?,
                    id,
                ],
            )?;
        }
        tx.commit()?;
        list(&db)
    }

    /// Deletes a skill. History that referenced it keeps only `{id, revision}`.
    ///
    /// # Errors
    /// `bad_request{field:"id", reason:"builtin"}`;
    /// `not_found{entity:"skill"}`; `internal`.
    pub fn delete(&self, id: &str) -> Result<Vec<SkillMeta>, KitError> {
        let db = self.lock()?;
        let origin: Option<SkillOrigin> = db
            .query_row("SELECT origin FROM skills WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        match origin {
            None => return Err(KitError::not_found("skill")),
            Some(SkillOrigin::Builtin) => {
                return Err(KitError::bad_request("id", "builtin"));
            }
            Some(SkillOrigin::Imported | SkillOrigin::Manual) => {}
        }
        db.execute("DELETE FROM skills WHERE id = ?1", [id])?;
        list(&db)
    }

    /// Pins the selected skills for one run, in the order given (docs/14 §5).
    ///
    /// # Errors
    /// `conflict{reason:"skill_changed", entity:<id>}` when a skill's revision
    /// differs or it was deleted, so the UI refreshes and asks the user to
    /// confirm again; `internal`.
    pub fn snapshot(
        &self,
        refs: &[SkillRef],
    ) -> Result<Vec<SkillSnapshot>, KitError> {
        let db = self.lock()?;
        let mut stmt = db.prepare_cached(
            "SELECT revision, name, body FROM skills WHERE id = ?1",
        )?;
        refs.iter()
            .map(|skill| {
                let row = stmt
                    .query_row([&skill.id], |row| {
                        Ok((row.get::<_, u32>(0)?, row.get(1)?, row.get(2)?))
                    })
                    .optional()?;
                match row {
                    Some((revision, name, body))
                        if revision == skill.revision => {
                        Ok(SkillSnapshot {
                            id: skill.id.clone(),
                            revision,
                            name,
                            body,
                        })
                    }
                    _ => Err(KitError::conflict("skill_changed")
                        .with_entity(&skill.id)),
                }
            })
            .collect()
    }

    fn lock(&self) -> Result<MutexGuard<'_, Db>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other(
                "skills store lock poisoned",
            )))
    }
}
#[path = "store_helpers.rs"]
mod helpers;
use helpers::*;


#[cfg(test)]
mod tests;
