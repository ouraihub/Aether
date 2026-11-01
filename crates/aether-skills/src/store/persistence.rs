use super::BuiltinSkill;
use super::META_COLUMNS;
use crate::content::{self, ContentIdentity};
use crate::dto::{SkillDto, SkillFileKind, SkillFileMeta, SkillMeta, SkillOrigin};
use crate::frontmatter;
use crate::package::{self, PackageFile};
use aether_support::KitError;
use aether_support::clock::now_millis;
use aether_support::id::new_id;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection as Db, OptionalExtension, Row, Transaction, params};
use std::collections::BTreeMap;

/// Fields that define a skill's content; `extra` is stored but not in the digest.
pub(crate) struct Content<'a> {
    pub(crate) name: &'a str,
    pub(crate) description: &'a str,
    pub(crate) version: Option<&'a str>,
    pub(crate) body: &'a str,
    pub(crate) license: Option<&'a str>,
    pub(crate) extra: &'a BTreeMap<String, String>,
    /// Packaged files; part of the digest.
    pub(crate) files: FileSource<'a>,
}

/// Where a skill's packaged files come from on insert or update.
#[derive(Clone, Copy)]
pub(crate) enum FileSource<'a> {
    Package(&'a [PackageFile]),
    /// The files currently stored for this skill id: kept on an edit, copied on a duplicate.
    Stored(&'a str),
}

impl<'a> Content<'a> {
    pub(crate) fn package(package: &'a package::Package) -> Self {
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

    pub(crate) fn identity(&self) -> ContentIdentity<'_> {
        ContentIdentity {
            name: self.name,
            description: self.description,
            version: self.version,
            body: self.body,
            license: self.license,
        }
    }
}

/// Reads stored hashes in the caller's write transaction; package hashes are borrowed.
pub(crate) fn content_digest(
    tx: &Transaction<'_>,
    content: &Content<'_>,
) -> Result<String, KitError> {
    match content.files {
        FileSource::Package(files) => Ok(content::digest(
            content.identity(),
            files
                .iter()
                .map(|file| (file.path.as_str(), file.sha256.as_str())),
        )),
        FileSource::Stored(id) => {
            let files: Vec<(String, String)> = tx
                .prepare_cached(
                    "SELECT path, sha256 FROM skill_files WHERE skill_id = ?1 ORDER BY path",
                )?
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            Ok(content::digest(
                content.identity(),
                files
                    .iter()
                    .map(|(path, sha256)| (path.as_str(), sha256.as_str())),
            ))
        }
    }
}

pub(crate) fn sync_builtin(db: &mut Db, builtin: &[BuiltinSkill]) -> Result<(), KitError> {
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
pub(crate) fn existing(
    db: &Db,
    name: &str,
) -> Result<Option<(String, Option<String>, u32)>, KitError> {
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
pub(crate) fn ensure_free(db: &Db, name: &str, except: Option<&str>) -> Result<(), KitError> {
    match existing(db, name)? {
        Some((id, ..)) if Some(id.as_str()) != except => Err(KitError::conflict("exists")),
        _ => Ok(()),
    }
}

pub(crate) fn insert(
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
            content_digest(tx, content)?,
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
pub(crate) fn update(
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
            content_digest(tx, content)?,
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

/// The caller owns the snapshot so instructions and files always share a revision.
pub(crate) fn get(db: &Db, id: &str) -> Result<SkillDto, KitError> {
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
        files: file_list(db, id)?,
    })
}

pub(crate) fn file_list(db: &Db, id: &str) -> Result<Vec<SkillFileMeta>, KitError> {
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

pub(crate) fn list(db: &Db) -> Result<Vec<SkillMeta>, KitError> {
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
