//! `mcp-client.db`: synchronous configuration, credentials and cache transactions (docs/27).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_support::db::{self, Migration};
use aether_support::{KitError, KitErrorCode, id, mask, net, secret};
use rusqlite::{OptionalExtension, Row, params};
use url::Url;

use crate::credentials::Secret;
use crate::http;
use crate::protocol::Tool;
use crate::session::McpTool;
use crate::{Approval, McpTransport, ServerDto, ServerSave, ServerSecret, ToolDto};

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: "CREATE TABLE servers (
  id TEXT PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL, url TEXT NOT NULL,
  secret_ciphertext BLOB,
  enabled INTEGER NOT NULL DEFAULT 0,
  trusted INTEGER NOT NULL DEFAULT 0,
  allow_private INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE tools (
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  name TEXT NOT NULL, description TEXT NOT NULL, input_schema TEXT NOT NULL,
  read_only INTEGER NOT NULL DEFAULT 0,
  approval TEXT NOT NULL DEFAULT 'ask' CHECK (approval IN ('auto','ask','deny')),
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY (server_id, name)
);",
    },
    Migration {
        version: 2,
        sql: "ALTER TABLE servers ADD COLUMN transport TEXT NOT NULL DEFAULT 'http'
  CHECK (transport IN ('http','stdio'));
ALTER TABLE servers ADD COLUMN command TEXT;
ALTER TABLE servers ADD COLUMN args TEXT;
ALTER TABLE servers ADD COLUMN cwd TEXT;",
    },
];

const MAX_COMMAND_CHARS: usize = 1024;
const MAX_ARGS: usize = 64;
const MAX_ARG_CHARS: usize = 4096;

const SERVER_COLUMNS: &str = "id, slug, name, url, secret_ciphertext, enabled, trusted, \
     allow_private, transport, command, args, cwd";

type Db = Mutex<rusqlite::Connection>;

/// MCP server configurations, credentials and discovered-tool cache.
pub struct Store {
    db: Db,
}

impl Store {
    /// Opens or creates `<root>/mcp-client.db`.
    ///
    /// # Errors
    /// `incompatible_db` for a newer schema; `internal` when SQLite fails.
    pub fn open(root: &Path) -> Result<Self, KitError> {
        let conn = db::open(&root.join("mcp-client.db"), MIGRATIONS)?;
        Ok(Self {
            db: Mutex::new(conn),
        })
    }

    /// All servers in display order.
    ///
    /// # Errors
    /// `internal` when SQLite fails.
    pub fn list(&self) -> Result<Vec<ServerDto>, KitError> {
        list(&*lock(&self.db)?)
    }

    /// Creates (no `id`) or updates a server and returns the new list.
    ///
    /// # Errors
    /// `bad_request` for an invalid slug, name, url (including credentials in the url:
    /// `userinfo`, `credential_query`), command, args, cwd or secret (a header, an env
    /// key, or credentials of the other transport); `bad_request(secret,
    /// transport_changed)` when the transport kind changes while the old credentials
    /// would be kept; `conflict{duplicate_slug}`; `not_found` for an unknown id;
    /// `unsupported`/`internal` when sealing fails, in which case nothing is changed.
    pub fn save(&self, input: ServerSave) -> Result<Vec<ServerDto>, KitError> {
        let ServerSave {
            id,
            slug,
            name,
            transport,
            secret: change,
            enabled,
            trusted,
            allow_private,
        } = input;
        validate_slug(&slug)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(KitError::bad_request("name", "empty"));
        }
        let stdio = matches!(transport, McpTransport::Stdio { .. });
        // Only HTTP reaches the network; stdio stores 0 (docs/27 §2).
        let allow_private = allow_private && !stdio;
        let columns = Columns::new(&transport, allow_private)?;
        // None = keep, Some(None) = clear, Some(Some) = replace.
        let sealed = change.map(|change| seal(change, stdio)).transpose()?;

        let mut conn = lock(&self.db)?;
        let tx = conn.transaction()?;
        let taken: Option<String> = tx
            .query_row("SELECT id FROM servers WHERE slug = ?1", [&slug], |row| {
                row.get(0)
            })
            .optional()?;
        if taken.is_some_and(|taken| Some(&taken) != id.as_ref()) {
            return Err(KitError::conflict("duplicate_slug").with_field("slug"));
        }
        let now = now_ms();
        if let Some(id) = id {
            let (old, old_kind) = stored_secret(&tx, &id)?;
            if old.is_some() && sealed.is_none() && old_kind != columns.kind {
                return Err(KitError::bad_request("secret", "transport_changed"));
            }
            tx.execute(
                "UPDATE servers SET slug = ?2, name = ?3, url = ?4, enabled = ?5, trusted = ?6,
                 allow_private = ?7, updated_at = ?8, transport = ?9, command = ?10, args = ?11,
                 cwd = ?12 WHERE id = ?1",
                params![
                    id,
                    slug,
                    name,
                    columns.url,
                    enabled,
                    trusted,
                    allow_private,
                    now,
                    columns.kind,
                    columns.command,
                    columns.args,
                    columns.cwd
                ],
            )?;
            if let Some(sealed) = sealed {
                if let Some(old) = old {
                    secret::forget(&old)?;
                }
                tx.execute(
                    "UPDATE servers SET secret_ciphertext = ?2 WHERE id = ?1",
                    params![id, sealed],
                )?;
            }
        } else {
            tx.execute(
                "INSERT INTO servers (id, slug, name, url, secret_ciphertext, enabled, trusted,
                 allow_private, position, created_at, updated_at, transport, command, args, cwd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM servers), ?9, ?9, ?10, ?11, ?12, ?13)",
                params![
                    id::new_id()?,
                    slug,
                    name,
                    columns.url,
                    sealed.flatten(),
                    enabled,
                    trusted,
                    allow_private,
                    now,
                    columns.kind,
                    columns.command,
                    columns.args,
                    columns.cwd
                ],
            )?;
        }
        tx.commit()?;
        list(&conn)
    }

    /// Deletes a server and its tools; returns the new list.
    ///
    /// # Errors
    /// `not_found` for an unknown id; `internal` when SQLite fails.
    pub fn delete(&self, id: &str) -> Result<Vec<ServerDto>, KitError> {
        let mut conn = lock(&self.db)?;
        let tx = conn.transaction()?;
        // Cascade deletion would skip `forget` (docs/10 §2).
        if let (Some(old), _) = stored_secret(&tx, id)? {
            secret::forget(&old)?;
        }
        tx.execute("DELETE FROM servers WHERE id = ?1", [id])?;
        tx.commit()?;
        list(&conn)
    }

    /// Sets one tool's approval default.
    ///
    /// # Errors
    /// `not_found` when the server has no such tool; `internal` when SQLite fails.
    pub fn set_tool_approval(
        &self,
        server_id: &str,
        tool: &str,
        approval: Approval,
    ) -> Result<(), KitError> {
        let changed = lock(&self.db)?.execute(
            "UPDATE tools SET approval = ?3 WHERE server_id = ?1 AND name = ?2",
            params![server_id, tool, approval_str(approval)],
        )?;
        if changed == 0 {
            return Err(KitError::not_found("tool"));
        }
        Ok(())
    }

    pub(crate) fn target(&self, id: &str) -> Result<Target, KitError> {
        load_target(&self.db, id)
    }

    pub(crate) fn active_targets(&self) -> Result<Vec<Target>, KitError> {
        active_targets(&self.db)
    }

    pub(crate) fn cached_tools(&self, id: &str) -> Result<Option<(i64, Vec<McpTool>)>, KitError> {
        cached_tools(&self.db, id)
    }

    pub(crate) fn replace_tools(
        &self,
        id: &str,
        tools: Vec<Tool>,
    ) -> Result<Vec<ToolDto>, KitError> {
        replace_tools(&self.db, id, tools)
    }
}

/// A server row needed to connect; the secret is opened only when connecting.
pub(crate) struct Target {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) name: String,
    /// Unmasked.
    pub(crate) transport: McpTransport,
    pub(crate) sealed: Option<Vec<u8>>,
    pub(crate) allow_private: bool,
}

impl Target {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            slug: row.get(1)?,
            name: row.get(2)?,
            transport: transport_from_row(row)?,
            sealed: row.get(4)?,
            allow_private: row.get(7)?,
        })
    }
}

/// The transport columns of a server row; an invalid combination is a corrupt row.
fn transport_from_row(row: &Row<'_>) -> rusqlite::Result<McpTransport> {
    let kind: String = row.get(8)?;
    let url: String = row.get(3)?;
    let command: Option<String> = row.get(9)?;
    let args: Option<String> = row.get(10)?;
    let cwd: Option<String> = row.get(11)?;
    let corrupt = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            format!("server row: {what}").into(),
        )
    };
    match (kind.as_str(), command, args) {
        ("http", None, None) if cwd.is_none() && !url.is_empty() => Ok(McpTransport::Http { url }),
        ("stdio", Some(command), Some(args)) if url.is_empty() => {
            let args = serde_json::from_str(&args).map_err(|_| corrupt("args"))?;
            Ok(McpTransport::Stdio { command, args, cwd })
        }
        _ => Err(corrupt("transport")),
    }
}

/// The columns `save` writes for a transport, after validating it (docs/27 §2).
struct Columns {
    kind: &'static str,
    /// Empty for stdio.
    url: String,
    command: Option<String>,
    /// JSON array.
    args: Option<String>,
    cwd: Option<String>,
}

impl Columns {
    fn new(transport: &McpTransport, allow_private: bool) -> Result<Self, KitError> {
        match transport {
            McpTransport::Http { url } => {
                check_server_url(url, allow_private)?;
                Ok(Self {
                    kind: "http",
                    url: url.clone(),
                    command: None,
                    args: None,
                    cwd: None,
                })
            }
            McpTransport::Stdio { command, args, cwd } => {
                check_stdio(command, args, cwd.as_deref())?;
                Ok(Self {
                    kind: "stdio",
                    url: String::new(),
                    command: Some(command.clone()),
                    args: Some(serde_json::to_string(args).map_err(KitError::internal)?),
                    cwd: cwd.clone(),
                })
            }
        }
    }
}

fn lock(db: &Db) -> Result<MutexGuard<'_, rusqlite::Connection>, KitError> {
    db.lock()
        .map_err(|_| KitError::new(KitErrorCode::Internal, "store lock poisoned"))
}

fn list(conn: &rusqlite::Connection) -> Result<Vec<ServerDto>, KitError> {
    let mut servers = conn.prepare(&format!(
        "SELECT {SERVER_COLUMNS} FROM servers ORDER BY position"
    ))?;
    let mut tools = conn.prepare(
        "SELECT name, description, read_only, approval FROM tools WHERE server_id = ?1 ORDER BY name",
    )?;
    let rows = servers.query_map([], |row| {
        Ok((
            Target::from_row(row)?,
            row.get::<_, bool>(5)?,
            row.get::<_, bool>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (target, enabled, trusted) = row?;
        let tools = tools
            .query_map([&target.id], |row| {
                Ok(ToolDto {
                    name: row.get(0)?,
                    description: row.get(1)?,
                    read_only: row.get(2)?,
                    approval: parse_approval(&row.get::<_, String>(3)?),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let transport = match target.transport {
            McpTransport::Http { url } => McpTransport::Http {
                url: mask::mask_url(&url),
            },
            stdio @ McpTransport::Stdio { .. } => stdio,
        };
        out.push(ServerDto {
            secret_keys: secret_keys(target.sealed.as_deref()),
            transport,
            id: target.id,
            slug: target.slug,
            name: target.name,
            enabled,
            trusted,
            allow_private: target.allow_private,
            tools,
        });
    }
    Ok(out)
}

/// Key names only. An unreadable secret lists no keys; connecting reports `key_unreadable`.
fn secret_keys(sealed: Option<&[u8]>) -> Vec<String> {
    sealed
        .and_then(|sealed| secret::open(sealed).ok())
        .and_then(|plain| serde_json::from_str::<Secret>(&plain).ok())
        .map(|secret| secret.keys())
        .unwrap_or_default()
}

/// The stored ciphertext and transport kind of an existing server.
fn stored_secret(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<(Option<Vec<u8>>, String), KitError> {
    conn.query_row(
        "SELECT secret_ciphertext, transport FROM servers WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| KitError::not_found("server"))
}

fn load_target(db: &Db, id: &str) -> Result<Target, KitError> {
    lock(db)?
        .query_row(
            &format!("SELECT {SERVER_COLUMNS} FROM servers WHERE id = ?1"),
            [id],
            Target::from_row,
        )
        .optional()?
        .ok_or_else(|| KitError::not_found("server"))
}

fn active_targets(db: &Db) -> Result<Vec<Target>, KitError> {
    let conn = lock(db)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {SERVER_COLUMNS} FROM servers WHERE enabled = 1 AND trusted = 1 ORDER BY position"
    ))?;
    let targets = stmt
        .query_map([], Target::from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(targets)
}

/// Cached tools and the oldest `fetched_at`; `None` when nothing was discovered yet.
// ponytail: a server with zero tools has no cache row and is re-discovered every run;
// add a `servers.fetched_at` column if that ever matters.
fn cached_tools(db: &Db, server_id: &str) -> Result<Option<(i64, Vec<McpTool>)>, KitError> {
    let conn = lock(db)?;
    let mut stmt = conn.prepare(
        "SELECT t.name, t.description, t.input_schema, t.approval, t.fetched_at, s.slug, s.name
         FROM tools t JOIN servers s ON s.id = t.server_id WHERE t.server_id = ?1 ORDER BY t.name",
    )?;
    let mut oldest = i64::MAX;
    let mut tools = Vec::new();
    let mut rows = stmt.query([server_id])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let slug: String = row.get(5)?;
        oldest = oldest.min(row.get(4)?);
        tools.push(McpTool {
            server: row.get(6)?,
            prefixed: format!("mcp_{slug}_{name}"),
            slug,
            name,
            description: row.get(1)?,
            schema: serde_json::from_str(&row.get::<_, String>(2)?).map_err(KitError::internal)?,
            approval: parse_approval(&row.get::<_, String>(3)?),
        });
    }
    Ok((!tools.is_empty()).then_some((oldest, tools)))
}

fn replace_tools(db: &Db, server_id: &str, tools: Vec<Tool>) -> Result<Vec<ToolDto>, KitError> {
    let mut conn = lock(db)?;
    let tx = conn.transaction()?;
    let mut dtos = Vec::with_capacity(tools.len());
    {
        let mut old =
            tx.prepare("SELECT approval FROM tools WHERE server_id = ?1 AND name = ?2")?;
        let mut kept = Vec::with_capacity(tools.len());
        for tool in &tools {
            let approval: Option<String> = old
                .query_row(params![server_id, tool.name], |row| row.get(0))
                .optional()?;
            kept.push(approval.map_or(Approval::Ask, |a| parse_approval(&a)));
        }
        tx.execute("DELETE FROM tools WHERE server_id = ?1", [server_id])?;
        let mut insert = tx.prepare(
            "INSERT INTO tools (server_id, name, description, input_schema, read_only, approval, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let now = now_ms();
        for (tool, approval) in tools.into_iter().zip(kept) {
            insert.execute(params![
                server_id,
                tool.name,
                tool.description,
                tool.schema,
                tool.read_only,
                approval_str(approval),
                now
            ])?;
            dtos.push(ToolDto {
                name: tool.name,
                description: tool.description,
                read_only: tool.read_only,
                approval,
            });
        }
    }
    tx.commit()?;
    tracing::info!(server_id, count = dtos.len(), "mcp.client.tools_refreshed");
    Ok(dtos)
}

/// Seals the new secret; `None` = cleared. Fails before anything is written. http takes
/// headers/query, stdio takes env; the other kind is rejected.
fn seal(input: ServerSecret, stdio: bool) -> Result<Option<Vec<u8>>, KitError> {
    let secret = Secret {
        headers: input.headers.unwrap_or_default(),
        query: input.query.unwrap_or_default(),
        env: input.env.unwrap_or_default(),
    };
    let foreign = if stdio {
        !secret.headers.is_empty() || !secret.query.is_empty()
    } else {
        !secret.env.is_empty()
    };
    if foreign {
        return Err(KitError::bad_request("secret", "transport_mismatch"));
    }
    if secret.is_empty() {
        return Ok(None);
    }
    secret.header_map()?;
    secret.check_env()?;
    let plain =
        zeroize::Zeroizing::new(serde_json::to_string(&secret).map_err(KitError::internal)?);
    secret::seal(&plain).map(Some)
}

/// The URL must pass the address policy and carry no credentials (docs/27 §2).
fn check_server_url(url: &str, allow_private: bool) -> Result<(), KitError> {
    let parsed = net::check_url(url, &http::policy(url, allow_private))?;
    // mask_url is the single definition of a credential-looking query: if it would
    // mask anything, the value belongs in `secret.query`.
    let masked = Url::parse(&mask::mask_url(url)).map_err(KitError::internal)?;
    if !parsed.query_pairs().eq(masked.query_pairs()) {
        return Err(KitError::bad_request("url", "credential_query"));
    }
    Ok(())
}

/// docs/27 §2: command non-empty, ≤ 1024 characters, single line; ≤ 64 args of ≤ 4096
/// characters; `cwd` an existing absolute directory. Args are listed unmasked, so a
/// credential-looking one is rejected: it belongs in `secret.env`.
pub(crate) fn check_stdio(
    command: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<(), KitError> {
    if command.trim().is_empty() {
        return Err(KitError::bad_request("command", "empty"));
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(KitError::bad_request("command", "too_long"));
    }
    if command.contains(['\n', '\r', '\0']) {
        return Err(KitError::bad_request("command", "invalid"));
    }
    if args.len() > MAX_ARGS {
        return Err(KitError::bad_request("args", "too_many"));
    }
    if args.iter().any(|arg| arg.chars().count() > MAX_ARG_CHARS) {
        return Err(KitError::bad_request("args", "too_long"));
    }
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err(KitError::bad_request("args", "invalid"));
    }
    // `--flag=value` is one token to the heuristic, so the parts are checked too.
    let secret_arg = |arg: &String| arg.split('=').any(mask::looks_like_secret);
    if args.iter().any(secret_arg) {
        return Err(KitError::bad_request("args", "credential_arg"));
    }
    if let Some(cwd) = cwd {
        check_cwd(cwd)?;
    }
    Ok(())
}

fn check_cwd(cwd: &str) -> Result<(), KitError> {
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err(KitError::bad_request("cwd", "not_absolute"));
    }
    if !path.is_dir() {
        return Err(KitError::bad_request("cwd", "not_found"));
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<(), KitError> {
    let valid = (1..=24).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(KitError::bad_request("slug", "invalid"))
    }
}

const fn approval_str(approval: Approval) -> &'static str {
    match approval {
        Approval::Auto => "auto",
        Approval::Ask => "ask",
        Approval::Deny => "deny",
    }
}

/// The CHECK constraint admits only the three values; anything else is treated as `ask`.
fn parse_approval(value: &str) -> Approval {
    match value {
        "auto" => Approval::Auto,
        "deny" => Approval::Deny,
        _ => Approval::Ask,
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
