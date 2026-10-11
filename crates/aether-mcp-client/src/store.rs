//! `mcp-client.db`: servers, sealed credentials, discovered tools (docs/13 §2, §4).

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_support::db::{self, Migration};
use aether_support::{KitError, KitErrorCode, id, mask, net, secret};
use rusqlite::{OptionalExtension, Row, params};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::protocol::{self, Connection, REQUEST_TIMEOUT, Secret};
use crate::session::{McpSession, McpTool, SessionServer};
use crate::{Approval, ServerDto, ServerSave, ServerSecret, ToolDto};

const MIGRATIONS: &[Migration] = &[Migration {
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
}];

/// Cached tools older than this are refreshed in the background (docs/13 §2).
const REFRESH_AFTER_MS: i64 = 10 * 60 * 1000;

const SERVER_COLUMNS: &str =
    "id, slug, name, url, secret_ciphertext, enabled, trusted, allow_private";

type Db = Arc<Mutex<rusqlite::Connection>>;

/// MCP server configurations, credentials and discovered tools.
pub struct Store {
    db: Db,
    pub(crate) timeout: Duration,
}

impl Store {
    /// Opens or creates `<root>/mcp-client.db`.
    ///
    /// # Errors
    /// `incompatible_db` for a newer schema; `internal` when SQLite fails.
    pub fn open(root: &Path) -> Result<Self, KitError> {
        let conn = db::open(&root.join("mcp-client.db"), MIGRATIONS)?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            timeout: REQUEST_TIMEOUT,
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
    /// `userinfo`, `credential_query`) or secret header; `conflict{duplicate_slug}`;
    /// `not_found` for an unknown id; `unsupported`/`internal` when sealing fails, in
    /// which case nothing is changed.
    pub fn save(&self, input: ServerSave) -> Result<Vec<ServerDto>, KitError> {
        let ServerSave {
            id,
            slug,
            name,
            url,
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
        check_server_url(&url, allow_private)?;
        // None = keep, Some(None) = clear, Some(Some) = replace.
        let sealed = change.map(seal).transpose()?;

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
            let old = stored_secret(&tx, &id)?;
            tx.execute(
                "UPDATE servers SET slug = ?2, name = ?3, url = ?4, enabled = ?5, trusted = ?6,
                 allow_private = ?7, updated_at = ?8 WHERE id = ?1",
                params![id, slug, name, url, enabled, trusted, allow_private, now],
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
                 allow_private, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM servers), ?9, ?9)",
                params![
                    id::new_id()?,
                    slug,
                    name,
                    url,
                    sealed.flatten(),
                    enabled,
                    trusted,
                    allow_private,
                    now
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
        if let Some(old) = stored_secret(&tx, id)? {
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

    /// Connects, handshakes and re-discovers the server's tools (the "test connection"
    /// action). Known tools keep their approval; new ones start at `ask`.
    ///
    /// # Errors
    /// `not_found`; `needs_config{key_unreadable}`; `bad_request(url)` for a url the
    /// policy rejects; `network`/`timeout`/`upstream`/`unauthorized`/`unsupported` from
    /// the server; `cancelled`.
    pub async fn refresh(
        &self,
        server_id: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<ToolDto>, KitError> {
        let target = load_target(&self.db, server_id)?;
        refresh_target(&self.db, &target, self.timeout, cancel)
            .await
            .map(|(_, tools)| tools)
    }

    /// The catalogue of enabled and trusted servers for one run. Cached tools are used
    /// when present; a missing cache is fetched now, a stale one is refreshed in the
    /// background. A server that cannot be read or reached is left out with a `warn`.
    /// `own_port` is this process's running MCP Server: a server pointing at it is
    /// skipped to avoid calling ourselves (docs/13 §9.2).
    pub async fn session(&self, cancel: &CancellationToken, own_port: Option<u16>) -> McpSession {
        let targets = match active_targets(&self.db) {
            Ok(targets) => targets,
            Err(error) => {
                tracing::warn!(code = ?error.code(), "mcp.client.session_failed");
                return McpSession::new(Vec::new(), self.timeout);
            }
        };
        let mut servers = Vec::new();
        for target in targets {
            if own_port.is_some_and(|port| target.is_local_port(port)) {
                tracing::warn!(server_id = %target.id, "mcp.client.self_server_skipped");
                continue;
            }
            match self.session_server(target, cancel).await {
                Ok(server) => servers.push(server),
                Err((server_id, error)) => {
                    tracing::warn!(server_id = %server_id, code = ?error.code(), "mcp.client.server_unavailable");
                }
            }
        }
        McpSession::new(servers, self.timeout)
    }

    async fn session_server(
        &self,
        target: Target,
        cancel: &CancellationToken,
    ) -> Result<SessionServer, (String, KitError)> {
        let cached = cached_tools(&self.db, &target.id).map_err(|e| (target.id.clone(), e))?;
        let (connection, tools) = match cached {
            Some((fetched_at, tools)) => {
                if now_ms() - fetched_at > REFRESH_AFTER_MS {
                    self.refresh_in_background(target.id.clone());
                }
                (None, tools)
            }
            None => {
                let (connection, _) = refresh_target(&self.db, &target, self.timeout, cancel)
                    .await
                    .map_err(|e| (target.id.clone(), e))?;
                let tools = cached_tools(&self.db, &target.id)
                    .map_err(|e| (target.id.clone(), e))?
                    .map(|(_, tools)| tools)
                    .unwrap_or_default();
                (Some(connection), tools)
            }
        };
        Ok(SessionServer {
            target,
            connection,
            tools,
        })
    }

    // ponytail: detached task with its own token; outlives the run but is bounded by the
    // request timeout. Owner-managed cancellation if hosts need clean shutdown here.
    fn refresh_in_background(&self, server_id: String) {
        let db = Arc::clone(&self.db);
        let timeout = self.timeout;
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            let result = match load_target(&db, &server_id) {
                Ok(target) => refresh_target(&db, &target, timeout, &cancel)
                    .await
                    .map(drop),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                tracing::warn!(server_id = %server_id, code = ?error.code(), "mcp.client.refresh_failed");
            }
        });
    }
}

/// A server row needed to connect; the secret is opened only when connecting.
pub(crate) struct Target {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) name: String,
    url: String,
    sealed: Option<Vec<u8>>,
    allow_private: bool,
}

impl Target {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            slug: row.get(1)?,
            name: row.get(2)?,
            url: row.get(3)?,
            sealed: row.get(4)?,
            allow_private: row.get(7)?,
        })
    }

    /// A loopback url on `port`: this process's own MCP Server when that runs there.
    fn is_local_port(&self, port: u16) -> bool {
        Url::parse(&self.url).is_ok_and(|url| {
            url.port_or_known_default() == Some(port)
                && match url.host() {
                    Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                    Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                    Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                    None => false,
                }
        })
    }

    /// Opens the secret and performs the handshake.
    pub(crate) async fn connect(
        &self,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Connection, KitError> {
        let secret = match &self.sealed {
            Some(sealed) => {
                let plain = secret::open(sealed)?;
                serde_json::from_str(&plain)
                    .map_err(|_| KitError::needs_config("key_unreadable"))?
            }
            None => Secret::default(),
        };
        Connection::open(&self.url, &secret, self.allow_private, timeout, cancel).await
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
        out.push(ServerDto {
            secret_keys: secret_keys(target.sealed.as_deref()),
            url: mask::mask_url(&target.url),
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

/// The stored ciphertext of an existing server.
fn stored_secret(conn: &rusqlite::Connection, id: &str) -> Result<Option<Vec<u8>>, KitError> {
    conn.query_row(
        "SELECT secret_ciphertext FROM servers WHERE id = ?1",
        [id],
        |row| row.get(0),
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

async fn refresh_target(
    db: &Db,
    target: &Target,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<(Connection, Vec<ToolDto>), KitError> {
    let mut connection = target.connect(timeout, cancel).await?;
    let tools = connection.list_tools(cancel).await?;
    let mut conn = lock(db)?;
    let tx = conn.transaction()?;
    let mut dtos = Vec::with_capacity(tools.len());
    {
        let mut old =
            tx.prepare("SELECT approval FROM tools WHERE server_id = ?1 AND name = ?2")?;
        let mut kept = Vec::with_capacity(tools.len());
        for tool in &tools {
            let approval: Option<String> = old
                .query_row(params![target.id, tool.name], |row| row.get(0))
                .optional()?;
            kept.push(approval.map_or(Approval::Ask, |a| parse_approval(&a)));
        }
        tx.execute("DELETE FROM tools WHERE server_id = ?1", [&target.id])?;
        let mut insert = tx.prepare(
            "INSERT INTO tools (server_id, name, description, input_schema, read_only, approval, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let now = now_ms();
        for (tool, approval) in tools.into_iter().zip(kept) {
            insert.execute(params![
                target.id,
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
    tracing::info!(server_id = %target.id, count = dtos.len(), "mcp.client.tools_refreshed");
    Ok((connection, dtos))
}

/// Seals the new secret; `None` = cleared. Fails before anything is written.
fn seal(input: ServerSecret) -> Result<Option<Vec<u8>>, KitError> {
    let secret = Secret {
        headers: input.headers.unwrap_or_default(),
        query: input.query.unwrap_or_default(),
    };
    if secret.is_empty() {
        return Ok(None);
    }
    secret.header_map()?;
    let plain =
        zeroize::Zeroizing::new(serde_json::to_string(&secret).map_err(KitError::internal)?);
    secret::seal(&plain).map(Some)
}

/// The url must pass the address policy and carry no credentials (docs/13 §4).
fn check_server_url(url: &str, allow_private: bool) -> Result<(), KitError> {
    let parsed = net::check_url(url, &protocol::policy(url, allow_private))?;
    // mask_url is the single definition of a credential-looking query: if it would
    // mask anything, the value belongs in `secret.query`.
    let masked = Url::parse(&mask::mask_url(url)).map_err(KitError::internal)?;
    if !parsed.query_pairs().eq(masked.query_pairs()) {
        return Err(KitError::bad_request("url", "credential_query"));
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
