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
    ManualSave, Preview, PreviewAction, PreviewCurrent, SkillDto, SkillFileKind, SkillFileMeta,
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
    "id, name, description, version, origin, revision, length(body), updated_at,
  EXISTS (SELECT 1 FROM skill_files f WHERE f.skill_id = skills.id AND f.kind = 'script'),
  (SELECT COUNT(*) FROM skill_files f WHERE f.skill_id = skills.id)";

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

/// Fields that define a skill's content; `extra` is stored but not in the digest.
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
    /// The files currently stored for this skill id: kept on an edit, copied on a duplicate.
    Stored(&'a str),
}

/// A file as the digest sees it.
struct FileHash {
    path: String,
    sha256: String,
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

    /// One skill with its body and file list, both read from one snapshot.
    ///
    /// # Errors
    /// `not_found{entity:"skill"}`; `internal` when SQLite fails.
    pub fn get(&self, id: &str) -> Result<SkillDto, KitError> {
        let mut db = self.lock()?;
        let db = db.transaction()?;
        let (meta, body, license, extra) = db
            .query_row(
                &format!("SELECT {META_COLUMNS}, body, license, extra FROM skills WHERE id = ?1"),
                [id],
                |row| {
                    Ok((
                        meta(row)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get::<_, String>(12)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("skill"))?;
        Ok(SkillDto {
            meta,
            body,
            license,
            extra: serde_json::from_str(&extra).map_err(KitError::internal)?,
            files: file_list(&db, id)?,
        })
    }

    /// The file list of revision `revision` of a skill. The revision check and the
    /// read share one snapshot, so another connection replacing the skill in between
    /// cannot hand out the newer list.
    ///
    /// # Errors
    /// `conflict{reason:"skill_changed", entity:<id>}` when the revision differs or the
    /// skill was deleted; `internal`.
    pub fn files(&self, id: &str, revision: u32) -> Result<Vec<SkillFileMeta>, KitError> {
        let mut db = self.lock()?;
        let db = db.transaction()?;
        ensure_revision(&db, id, revision)?;
        file_list(&db, id)
    }

    /// Content of one packaged file of revision `revision`. Nothing is executed or
    /// written to disk; running scripts is the host's decision. The revision check and
    /// the read share one snapshot, as in [`Store::files`].
    ///
    /// # Errors
    /// `conflict{reason:"skill_changed", entity:<id>}` when the revision differs or the
    /// skill was deleted; `not_found{entity:"skill_file"}` for an unknown path; `internal`.
    pub fn file(&self, id: &str, revision: u32, path: &str) -> Result<Vec<u8>, KitError> {
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
    /// The package errors of docs/14 §2; `conflict{reason:"builtin_name"}`; `internal`.
    pub fn preview(&self, source: &Source) -> Result<Preview, KitError> {
        let package = package::read(source)?;
        let db = self.lock()?;
        let current = existing(&db, &package.skill.name)?
            .map(|(_, version, revision)| PreviewCurrent { version, revision });
        let digest = Content::package(&package).digest(&db)?;
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
        let content = Content::package(&package);
        let mut db = self.lock()?;
        // Passing validation again is not approval: only the previewed bytes are (docs/14 §7.1).
        if content.digest(&db)? != confirmed_digest {
            return Err(KitError::conflict("content_changed"));
        }
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
            let content = input.content(&extra, None, FileSource::Package(&[]));
            insert(&tx, &content, SkillOrigin::Manual)?;
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
        let content = input.content(&extra, license.as_deref(), FileSource::Stored(id));
        update(&tx, id, &content, origin)?;
        tx.commit()?;
        list(&db)
    }

    /// Copies any skill (typically a built-in) as a `manual` skill named `new_name`,
    /// packaged files included.
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
            files: FileSource::Stored(id),
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
        files: FileSource<'a>,
    ) -> Content<'a> {
        Content {
            name: &self.name,
            description: &self.description,
            version: self.version.as_deref(),
            body: &self.body,
            license,
            extra,
            files,
        }
    }
}

impl<'a> Content<'a> {
    fn package(package: &'a package::Package) -> Self {
        Self {
            name: &package.skill.name,
            description: &package.skill.description,
            version: package.skill.version.as_deref(),
            body: &package.skill.body,
            license: package.license.as_deref(),
            extra: &package.skill.extra,
            files: FileSource::Package(&package.files),
        }
    }

    fn digest(&self, db: &Db) -> Result<String, KitError> {
        let files = match self.files {
            FileSource::Package(files) => files
                .iter()
                .map(|file| FileHash {
                    path: file.path.clone(),
                    sha256: file.sha256.clone(),
                })
                .collect(),
            FileSource::Stored(id) => db
                .prepare_cached(
                    "SELECT path, sha256 FROM skill_files WHERE skill_id = ?1 ORDER BY path",
                )?
                .query_map([id], |row| {
                    Ok(FileHash {
                        path: row.get(0)?,
                        sha256: row.get(1)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(digest(
            [
                Some(self.name),
                Some(self.description),
                self.version,
                Some(self.body),
            ],
            self.license,
            &files,
        ))
    }
}

/// `sha256` of the text fields, each length-prefixed so moving text between fields changes
/// the digest (`None` and `Some("")` differ), then — only when the skill has a license or
/// files — the license and every `(path, sha256)` in path order. Skills without either
/// keep the migration 1 digest, so upgrading does not bump their revision.
fn digest(text: [Option<&str>; 4], license: Option<&str>, files: &[FileHash]) -> String {
    let mut hasher = Sha256::new();
    let mut field = |value: Option<&str>| match value {
        Some(text) => {
            hasher.update([1]);
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        None => hasher.update([0]),
    };
    text.into_iter().for_each(&mut field);
    if license.is_some() || !files.is_empty() {
        field(Some("package"));
        field(license);
        for file in files {
            field(Some(&file.path));
            field(Some(&file.sha256));
        }
    }
    hex(&hasher.finalize())
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
            files: FileSource::Package(&[]),
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
    let id = new_id()?;
    tx.execute(
        "INSERT INTO skills (id, name, description, version, body, license, extra, origin,
           revision, digest, position, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9,
           (SELECT COALESCE(MAX(position) + 1, 0) FROM skills), ?10, ?10)",
        params![
            id,
            content.name,
            content.description,
            content.version,
            content.body,
            content.license,
            serde_json::to_string(content.extra).map_err(KitError::internal)?,
            origin_str(origin),
            content.digest(tx)?,
            now,
        ],
    )?;
    match content.files {
        FileSource::Package(files) => write_files(tx, &id, files),
        FileSource::Stored(from) => {
            tx.execute(
                "INSERT INTO skill_files (skill_id, path, kind, size, sha256, content)
                 SELECT ?1, path, kind, size, sha256, content FROM skill_files WHERE skill_id = ?2",
                [&id, from],
            )?;
            Ok(())
        }
    }
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
            content.digest(tx)?,
            now_millis(),
        ],
    )?;
    match content.files {
        FileSource::Package(files) => {
            tx.execute("DELETE FROM skill_files WHERE skill_id = ?1", [id])?;
            write_files(tx, id, files)
        }
        // Kept as stored: an edit of the text never touches the files.
        FileSource::Stored(_) => Ok(()),
    }
}

fn write_files(tx: &Transaction<'_>, id: &str, files: &[PackageFile]) -> Result<(), KitError> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO skill_files (skill_id, path, kind, size, sha256, content)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for file in files {
        stmt.execute(params![
            id,
            file.path,
            kind_str(file.kind),
            i64::try_from(file.content.len()).map_err(KitError::internal)?,
            file.sha256,
            file.content,
        ])?;
    }
    Ok(())
}

fn file_list(db: &Db, id: &str) -> Result<Vec<SkillFileMeta>, KitError> {
    let mut stmt = db.prepare_cached(
        "SELECT path, kind, size FROM skill_files WHERE skill_id = ?1 ORDER BY path",
    )?;
    let rows = stmt
        .query_map([id], |row| {
            Ok(SkillFileMeta {
                path: row.get(0)?,
                kind: row.get(1)?,
                // Files are capped at 1 MiB, so the size always fits u32.
                size: row.get::<_, u32>(2)?.into(),
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// `conflict{reason:"skill_changed", entity:<id>}` unless `id` exists at `revision`.
fn ensure_revision(db: &Db, id: &str, revision: u32) -> Result<(), KitError> {
    let current: Option<u32> = db
        .query_row("SELECT revision FROM skills WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    if current == Some(revision) {
        Ok(())
    } else {
        Err(KitError::conflict("skill_changed").with_entity(id))
    }
}

/// Lowercase hex of a hash.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
        has_scripts: row.get(8)?,
        file_count: row.get(9)?,
    })
}

fn origin_str(origin: SkillOrigin) -> &'static str {
    match origin {
        SkillOrigin::Imported => "imported",
        SkillOrigin::Manual => "manual",
        SkillOrigin::Builtin => "builtin",
    }
}

fn kind_str(kind: SkillFileKind) -> &'static str {
    match kind {
        SkillFileKind::Script => "script",
        SkillFileKind::Reference => "reference",
        SkillFileKind::Asset => "asset",
    }
}

impl FromSql for SkillFileKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "script" => Ok(Self::Script),
            "reference" => Ok(Self::Reference),
            "asset" => Ok(Self::Asset),
            _ => Err(FromSqlError::InvalidType),
        }
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
