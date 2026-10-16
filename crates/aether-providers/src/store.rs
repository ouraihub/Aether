//! `providers.db`: whole-provider save, key tri-state and connection snapshots (docs/11 §2–§4).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_support::db::{self, Migration};
use aether_support::id::new_id;
use aether_support::mask::mask_secret;
use aether_support::secret::{self, KeyChange};
use aether_support::{Connection, KitError, Protocol};
use rusqlite::{Connection as Db, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::dto::{
    Endpoint, EndpointSave, Model, ModelFilter, ModelOption, ModelRef, ModelType, Provider,
    ProviderSave, TypeSource,
};
use crate::{api_base, infer_reasoning};

const MIGRATIONS: &[Migration] = &[
Migration {
    version: 1,
    sql: "
CREATE TABLE providers (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE endpoints (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  slug TEXT NOT NULL, base_url TEXT NOT NULL,
  protocol TEXT NOT NULL CHECK (protocol IN ('anthropic-messages','openai-chat','openai-responses')),
  key_ciphertext BLOB, position INTEGER NOT NULL,
  UNIQUE (provider_id, slug)
);
CREATE TABLE models (
  endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
  model_id TEXT NOT NULL,
  model_type TEXT NOT NULL CHECK (model_type IN ('chat','image','video','tts','stt','embedding','rerank','unknown')),
  type_source TEXT NOT NULL CHECK (type_source IN ('inferred','user')),
  context_window INTEGER, max_output INTEGER,
  can_vision INTEGER NOT NULL DEFAULT 0,
  supports_tools INTEGER,
  position INTEGER NOT NULL,
  PRIMARY KEY (endpoint_id, model_id)
);",
},
Migration { version: 2, sql: "ALTER TABLE models ADD COLUMN reasoning TEXT;" },
];

const MAX_NAME_CHARS: usize = 80;
const MAX_ENDPOINTS: usize = 20;
const MAX_MODELS: usize = 1000;
const MAX_SLUG: usize = 40;
const MAX_BASE_URL: usize = 2048;

/// Seals a plaintext key; a parameter only so tests can make sealing fail.
type Seal = fn(&str) -> Result<Vec<u8>, KitError>;

/// Stored base URL, protocol and plaintext key of one endpoint.
pub(crate) type EndpointRow = (String, Protocol, Option<Zeroizing<String>>);

/// Provider configuration stored in `<root>/providers.db`.
pub struct Store {
    db: Mutex<Db>,
}

impl Store {
    /// Opens (creating and migrating) `<root>/providers.db`.
    ///
    /// # Errors
    /// `incompatible_db` for a newer schema; `internal` when SQLite fails.
    pub fn open(root: &Path) -> Result<Self, KitError> {
        let path = root.join("providers.db");
        // The first native-search release added this column without advancing v2.
        let has_native = if path.exists() {
            let existing = Db::open(&path)?;
            existing.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('models') WHERE name = 'native_web_search')",
                [],
                |row| row.get::<_, bool>(0),
            )?
        } else {
            false
        };
        let steps = [
            MIGRATIONS[0],
            MIGRATIONS[1],
            Migration {
                version: 3,
                sql: if has_native {
                    ""
                } else {
                    "ALTER TABLE models ADD COLUMN native_web_search INTEGER NOT NULL DEFAULT 0;"
                },
            },
            Migration {
                version: 4,
                sql: "ALTER TABLE providers ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;",
            },
        ];
        let db = db::open(&path, &steps)?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// All providers with endpoints and models in display order.
    ///
    /// # Errors
    /// `internal` when SQLite fails or stored data is malformed.
    pub fn list(&self) -> Result<Vec<Provider>, KitError> {
        list(&*self.lock()?)
    }

    /// Replaces one provider as a whole (docs/11 §3): endpoints missing from the request
    /// are deleted with their models; keys follow the tri-state of `apiKey`.
    ///
    /// # Errors
    /// `bad_request` for invalid input; `not_found` for an unknown provider or endpoint id;
    /// the sealing error (e.g. `unsupported`) when a new key cannot be encrypted, in which
    /// case nothing is written.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<Vec<Provider>, KitError> {
        let db = self.lock()?;
        let changed = db.execute(
            "UPDATE providers SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, enabled, now_millis()],
        )?;
        if changed == 0 {
            return Err(KitError::not_found("provider"));
        }
        list(&db)
    }

    pub fn save(&self, input: ProviderSave) -> Result<Vec<Provider>, KitError> {
        self.save_with(input, secret::seal)
    }

    /// Deletes a provider with its endpoints and models.
    ///
    /// # Errors
    /// `not_found` for an unknown id; `internal` when SQLite fails.
    pub fn delete(&self, id: &str) -> Result<Vec<Provider>, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        // The cascade does not call `secret::forget`, so collect ciphertexts first (docs/10 §2).
        let stale = tx
            .prepare(
                "SELECT key_ciphertext FROM endpoints
                 WHERE provider_id = ?1 AND key_ciphertext IS NOT NULL",
            )?
            .query_map([id], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if tx.execute("DELETE FROM providers WHERE id = ?1", [id])? == 0 {
            return Err(KitError::not_found("provider"));
        }
        tx.commit()?;
        forget_all(&stale)?;
        list(&db)
    }

    /// Every stored model matching `filter`, for model pickers.
    ///
    /// # Errors
    /// `internal` when SQLite fails or stored data is malformed.
    pub fn model_options(&self, filter: ModelFilter) -> Result<Vec<ModelOption>, KitError> {
        let db = self.lock()?;
        let mut stmt = db.prepare_cached(
            "SELECT p.name, e.slug, m.endpoint_id, m.model_id, m.model_type, m.max_output, m.supports_tools, m.reasoning, m.native_web_search, e.protocol, m.can_vision
             FROM models m JOIN endpoints e ON e.id = m.endpoint_id JOIN providers p ON p.id = e.provider_id
             WHERE p.enabled = 1
             ORDER BY p.position, p.id, e.position, m.position",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<u32>>(5)?,
                    row.get::<_, Option<bool>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, bool>(10)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut options = Vec::new();
        for (
            provider,
            slug,
            endpoint_id,
            model_id,
            model_type,
            max_output,
            supports_tools,
            reasoning,
            native_web_search,
            protocol,
            can_vision,
        ) in rows
        {
            let model_type = ModelType::parse(&model_type).ok_or_else(|| corrupt("model_type"))?;
            let protocol = parse_protocol(&protocol)?;
            let reasoning = reasoning
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(KitError::internal)?
                .or_else(|| {
                    (model_type == ModelType::Chat)
                        .then(|| infer_reasoning(&model_id, protocol, max_output))
                        .flatten()
                });
            if filter.model_type.is_some_and(|wanted| wanted != model_type)
                || filter
                    .supports_tools
                    .is_some_and(|wanted| supports_tools != Some(wanted))
            {
                continue;
            }
            options.push(ModelOption {
                label: format!("{provider} · {slug} · {model_id}"),
                endpoint_id,
                model_id,
                model_type,
                supports_tools,
                reasoning,
                native_web_search,
                can_vision,
            });
        }
        Ok(options)
    }

    /// The connection snapshot for one stored model. `base_url` is [`api_base`] of the
    /// stored address; `api_key` is `None` when no key is stored.
    ///
    /// # Errors
    /// `not_found{entity:"model"}`; `needs_config{reason:"key_unreadable"}` when the stored
    /// key cannot be decrypted; `internal` when SQLite fails.
    pub fn resolve(&self, model: &ModelRef) -> Result<Connection, KitError> {
        let db = self.lock()?;
        let row = db.query_row(
            "SELECT e.base_url, e.protocol, e.key_ciphertext, m.model_type, m.context_window, m.max_output, m.supports_tools, m.reasoning, m.native_web_search, m.can_vision
             FROM models m JOIN endpoints e ON e.id = m.endpoint_id JOIN providers p ON p.id = e.provider_id
             WHERE m.endpoint_id = ?1 AND m.model_id = ?2 AND p.enabled = 1",
            params![model.endpoint_id, model.model_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, String>(3)?, row.get::<_, Option<u32>>(4)?, row.get::<_, Option<u32>>(5)?, row.get::<_, Option<bool>>(6)?, row.get::<_, Option<String>>(7)?, row.get::<_, bool>(8)?, row.get::<_, bool>(9)?)),
        ).optional()?;
        let (
            base_url,
            protocol,
            key,
            model_type,
            context_window,
            max_output,
            supports_tools,
            reasoning,
            native_web_search,
            can_vision,
        ) = row.ok_or_else(|| KitError::not_found("model"))?;
        let protocol = parse_protocol(&protocol)?;
        let model_type = ModelType::parse(&model_type).ok_or_else(|| corrupt("model_type"))?;
        let reasoning = reasoning
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(KitError::internal)?
            .or_else(|| {
                (model_type == ModelType::Chat)
                    .then(|| infer_reasoning(&model.model_id, protocol, max_output))
                    .flatten()
            });
        Ok(Connection {
            base_url: api_base(&base_url),
            protocol,
            api_key: open_key(key.as_deref())?,
            model_id: Some(model.model_id.clone()),
            endpoint_id: Some(model.endpoint_id.clone()),
            context_window,
            max_output,
            supports_tools,
            reasoning,
            native_web_search,
            can_vision,
        })
    }

    /// The connection snapshot of an endpoint without a model.
    /// # Errors
    /// Reports missing endpoints, unreadable credentials or storage failures.
    pub fn endpoint_connection(&self, endpoint_id: &str) -> Result<Connection, KitError> {
        let (base_url, protocol, api_key) = self.endpoint(endpoint_id)?;
        Ok(Connection {
            base_url: api_base(&base_url),
            protocol,
            api_key,
            model_id: None,
            endpoint_id: Some(endpoint_id.to_owned()),
            context_window: None,
            max_output: None,
            supports_tools: None,
            reasoning: None,
            native_web_search: false,
            can_vision: false,
        })
    }

    /// The stored (not normalised) base URL, protocol and opened key of one endpoint.
    pub(crate) fn endpoint(&self, endpoint_id: &str) -> Result<EndpointRow, KitError> {
        let db = self.lock()?;
        let row = db
            .query_row(
                "SELECT base_url, protocol, key_ciphertext FROM endpoints WHERE id = ?1",
                [endpoint_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                },
            )
            .optional()?;
        let (base_url, protocol, key) = row.ok_or_else(|| KitError::not_found("endpoint"))?;
        Ok((
            base_url,
            parse_protocol(&protocol)?,
            open_key(key.as_deref())?,
        ))
    }

    fn save_with(&self, input: ProviderSave, seal: Seal) -> Result<Vec<Provider>, KitError> {
        validate(&input)?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let now = now_millis();
        let provider_id = if let Some(id) = &input.id {
            let sql = "UPDATE providers SET name = ?2, updated_at = ?3, enabled = COALESCE(?4, enabled) WHERE id = ?1";
            if tx.execute(sql, params![id, input.name, now, input.enabled])? == 0 {
                return Err(KitError::not_found("provider"));
            }
            id.clone()
        } else {
            let id = new_id()?;
            tx.execute(
                "INSERT INTO providers (id, name, position, created_at, updated_at, enabled)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(position) + 1, 0) FROM providers), ?3, ?3, ?4)",
                params![id, input.name, now, input.enabled.unwrap_or(true)],
            )?;
            id
        };

        let mut old_keys: HashMap<String, Option<Vec<u8>>> = tx
            .prepare("SELECT id, key_ciphertext FROM endpoints WHERE provider_id = ?1")?
            .query_map([&provider_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let kept: HashSet<&str> = input
            .endpoints
            .iter()
            .filter_map(|e| e.id.as_deref())
            .collect();
        if kept.iter().any(|id| !old_keys.contains_key(*id)) {
            return Err(KitError::not_found("endpoint"));
        }
        // Ciphertexts to `forget` once the new state is committed (docs/10 §2).
        let mut stale = Vec::new();
        for (id, key) in &old_keys {
            if !kept.contains(id.as_str()) {
                tx.execute("DELETE FROM endpoints WHERE id = ?1", [id])?;
                stale.extend(key.clone());
            }
        }
        // Kept endpoints may swap slugs; park them on their unique ids first so the
        // UNIQUE (provider_id, slug) check never sees a transient duplicate.
        tx.execute(
            "UPDATE endpoints SET slug = id WHERE provider_id = ?1",
            [&provider_id],
        )?;

        for (position, mut endpoint) in input.endpoints.into_iter().enumerate() {
            let old_key = endpoint
                .id
                .as_ref()
                .and_then(|id| old_keys.remove(id))
                .flatten();
            let key = match KeyChange::from_field(endpoint.api_key.take()) {
                KeyChange::Keep => old_key,
                KeyChange::Clear => {
                    stale.extend(old_key);
                    None
                }
                // A failing seal returns here; dropping `tx` rolls everything back.
                KeyChange::Set(plain) => {
                    let sealed = seal(&plain)?;
                    stale.extend(old_key);
                    Some(sealed)
                }
            };
            write_endpoint(&tx, &provider_id, position, &endpoint, key.as_deref())?;
        }
        tx.commit()?;
        forget_all(&stale)?;
        list(&db)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Db>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("providers store lock poisoned")))
    }
}

fn validate(input: &ProviderSave) -> Result<(), KitError> {
    let name_chars = input.name.trim().chars().count();
    if name_chars == 0 || name_chars > MAX_NAME_CHARS {
        return Err(too_long("name", MAX_NAME_CHARS));
    }
    if input.endpoints.len() > MAX_ENDPOINTS {
        return Err(too_long("endpoints", MAX_ENDPOINTS));
    }
    let mut slugs = HashSet::new();
    let mut ids = HashSet::new();
    for endpoint in &input.endpoints {
        let slug = &endpoint.slug;
        if slug.is_empty()
            || slug.len() > MAX_SLUG
            || !slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(KitError::bad_request("slug", "invalid_slug"));
        }
        if !slugs.insert(slug.as_str()) {
            return Err(KitError::bad_request("slug", "duplicate"));
        }
        if endpoint
            .id
            .as_ref()
            .is_some_and(|id| !ids.insert(id.as_str()))
        {
            return Err(KitError::bad_request("id", "duplicate"));
        }
        let url = endpoint.base_url.trim();
        if url.len() > MAX_BASE_URL {
            return Err(too_long("baseUrl", MAX_BASE_URL));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(KitError::bad_request("baseUrl", "invalid_url"));
        }
        if endpoint.models.len() > MAX_MODELS {
            return Err(too_long("models", MAX_MODELS));
        }
        let mut models = HashSet::new();
        for model in &endpoint.models {
            if model.native_web_search
                && (model.model_type != ModelType::Chat
                    || endpoint.protocol == Protocol::OpenaiChat)
            {
                return Err(KitError::bad_request(
                    "nativeWebSearch",
                    "unsupported_protocol",
                ));
            }
            if let Some(Some(config)) = &model.reasoning
                && (model.model_type != ModelType::Chat
                    || !config.validate(endpoint.protocol, model.max_output))
            {
                return Err(KitError::bad_request("reasoning", "invalid"));
            }
            if model.id.is_empty() {
                return Err(KitError::bad_request("models", "empty_id"));
            }
            if !models.insert(model.id.as_str()) {
                return Err(KitError::bad_request("models", "duplicate"));
            }
        }
    }
    Ok(())
}

fn too_long(field: &str, limit: usize) -> KitError {
    KitError::bad_request(field, "out_of_range")
        .with_limit(u64::try_from(limit).unwrap_or(u64::MAX))
}

fn write_endpoint(
    tx: &Db,
    provider_id: &str,
    position: usize,
    endpoint: &EndpointSave,
    key: Option<&[u8]>,
) -> Result<(), KitError> {
    let position = i64::try_from(position).unwrap_or(i64::MAX);
    let base_url = endpoint.base_url.trim();
    let protocol = protocol_str(endpoint.protocol);
    let id = if let Some(id) = &endpoint.id {
        tx.execute("UPDATE endpoints SET slug = ?2, base_url = ?3, protocol = ?4, key_ciphertext = ?5, position = ?6 WHERE id = ?1", params![id, endpoint.slug, base_url, protocol, key, position])?;
        id.clone()
    } else {
        let id = new_id()?;
        tx.execute("INSERT INTO endpoints (id, provider_id, slug, base_url, protocol, key_ciphertext, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![id, provider_id, endpoint.slug, base_url, protocol, key, position])?;
        id
    };
    let old: HashMap<String, Option<String>> = tx
        .prepare("SELECT model_id, reasoning FROM models WHERE endpoint_id = ?1")?
        .query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    tx.execute("DELETE FROM models WHERE endpoint_id = ?1", [&id])?;
    let mut insert = tx.prepare_cached("INSERT INTO models (endpoint_id, model_id, model_type, type_source, context_window, max_output, can_vision, supports_tools, position, reasoning, native_web_search) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)")?;
    for (position, model) in endpoint.models.iter().enumerate() {
        let reasoning = match &model.reasoning {
            Some(Some(config)) => Some(serde_json::to_string(config).map_err(KitError::internal)?),
            Some(None) => None,
            None => old.get(&model.id).cloned().flatten(),
        };
        insert.execute(params![
            id,
            model.id,
            model.model_type.as_str(),
            model.type_source.as_str(),
            model.context_window,
            model.max_output,
            model.can_vision,
            model.supports_tools,
            i64::try_from(position).unwrap_or(i64::MAX),
            reasoning,
            model.native_web_search
        ])?;
    }
    Ok(())
}

fn list(db: &Db) -> Result<Vec<Provider>, KitError> {
    let mut models: HashMap<String, Vec<Model>> = HashMap::new();
    let mut stmt = db.prepare_cached("SELECT m.endpoint_id, m.model_id, m.model_type, m.type_source, m.context_window, m.max_output, m.can_vision, m.supports_tools, m.reasoning, m.native_web_search, e.protocol FROM models m JOIN endpoints e ON e.id = m.endpoint_id ORDER BY m.endpoint_id, m.position")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let model_type: String = row.get(2)?;
        let type_source: String = row.get(3)?;
        let reasoning: Option<String> = row.get(8)?;
        let protocol = parse_protocol(&row.get::<_, String>(10)?)?;
        let model_type = ModelType::parse(&model_type).ok_or_else(|| corrupt("model_type"))?;
        let max_output = row.get(5)?;
        let model_id: String = row.get(1)?;
        models.entry(row.get(0)?).or_default().push(Model {
            id: model_id.clone(),
            model_type,
            type_source: TypeSource::parse(&type_source).ok_or_else(|| corrupt("type_source"))?,
            context_window: row.get(4)?,
            max_output: row.get(5)?,
            can_vision: row.get(6)?,
            supports_tools: row.get(7)?,
            reasoning: reasoning
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(KitError::internal)?
                .or_else(|| {
                    (model_type == ModelType::Chat)
                        .then(|| infer_reasoning(&model_id, protocol, max_output))
                        .flatten()
                }),
            native_web_search: row.get(9)?,
        });
    }
    let mut endpoints: HashMap<String, Vec<Endpoint>> = HashMap::new();
    let mut stmt = db.prepare_cached("SELECT provider_id, id, slug, base_url, protocol, key_ciphertext FROM endpoints ORDER BY provider_id, position")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(1)?;
        let base_url: String = row.get(3)?;
        let protocol: String = row.get(4)?;
        let key: Option<Vec<u8>> = row.get(5)?;
        let plain = key.as_deref().map(secret::open).and_then(Result::ok);
        endpoints.entry(row.get(0)?).or_default().push(Endpoint {
            slug: row.get(2)?,
            protocol: parse_protocol(&protocol)?,
            has_key: key.is_some(),
            key_masked: plain.as_ref().map(|plain| mask_secret(plain)),
            key_readable: key.is_none() || plain.is_some(),
            probe_url: format!("{}/models", api_base(&base_url)),
            models: models.remove(&id).unwrap_or_default(),
            base_url,
            id,
        });
    }
    let mut stmt =
        db.prepare_cached("SELECT id, name, enabled FROM providers ORDER BY position, id")?;
    let providers = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(providers
        .into_iter()
        .map(|(id, name, enabled)| Provider {
            enabled,
            endpoints: endpoints.remove(&id).unwrap_or_default(),
            id,
            name,
        })
        .collect())
}

fn open_key(key: Option<&[u8]>) -> Result<Option<Zeroizing<String>>, KitError> {
    key.map(secret::open).transpose()
}

fn forget_all(stale: &[Vec<u8>]) -> Result<(), KitError> {
    stale.iter().try_for_each(|sealed| secret::forget(sealed))
}

const fn protocol_str(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::AnthropicMessages => "anthropic-messages",
        Protocol::OpenaiChat => "openai-chat",
        Protocol::OpenaiResponses => "openai-responses",
    }
}

fn parse_protocol(text: &str) -> Result<Protocol, KitError> {
    match text {
        "anthropic-messages" => Ok(Protocol::AnthropicMessages),
        "openai-chat" => Ok(Protocol::OpenaiChat),
        "openai-responses" => Ok(Protocol::OpenaiResponses),
        _ => Err(corrupt("protocol")),
    }
}

fn corrupt(column: &str) -> KitError {
    KitError::internal(std::io::Error::other(format!(
        "providers.db: bad {column} value"
    )))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use aether_support::KitErrorCode;
    use serde_json::{Value, json};

    use super::*;

    const KEY: &str = "sk-test-0123456789abcd";

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn request(value: Value) -> ProviderSave {
        serde_json::from_value(value).unwrap()
    }

    fn model(id: &str) -> Value {
        json!({ "id": id, "modelType": "chat", "typeSource": "inferred", "contextWindow": 200_000,
                "maxOutput": null, "canVision": true, "supportsTools": true })
    }

    fn endpoint(id: Option<&str>, slug: &str, models: &[&str]) -> Value {
        let mut value = json!({ "slug": slug, "baseUrl": format!("https://{slug}.example.com/"),
            "protocol": "openai-chat", "models": models.iter().map(|m| model(m)).collect::<Vec<_>>() });
        if let Some(id) = id {
            value["id"] = json!(id);
        }
        value
    }

    fn key_of(store: &Store, endpoint_id: &str) -> Option<String> {
        let connection = store.endpoint_connection(endpoint_id).unwrap();
        connection.api_key.map(|key| key.as_str().to_owned())
    }

    fn fail_seal(_: &str) -> Result<Vec<u8>, KitError> {
        Err(KitError::unsupported("secret_store"))
    }

    #[test]
    fn saved_null_reasoning_is_inferred_consistently_without_reconfiguration() {
        let (dir, store) = store();
        let mut ep = endpoint(
            None,
            "auto",
            &["claude-opus-5-5", "claude-sonnet-5-5", "unknown"],
        );
        ep["protocol"] = json!("anthropic-messages");
        let saved = store
            .save(request(json!({"name":"Automatic","endpoints":[ep]})))
            .unwrap();
        let id = &saved[0].endpoints[0].id;
        let reference = ModelRef {
            endpoint_id: id.clone(),
            model_id: "claude-sonnet-5-5".into(),
        };
        let expected =
            infer_reasoning(&reference.model_id, Protocol::AnthropicMessages, None).unwrap();
        assert_eq!(
            saved[0].endpoints[0].models[1].reasoning.as_ref(),
            Some(&expected)
        );
        assert_eq!(
            store.resolve(&reference).unwrap().reasoning,
            Some(expected.clone())
        );
        let options = store.model_options(ModelFilter::default()).unwrap();
        assert!(
            !options[0]
                .reasoning
                .as_ref()
                .unwrap()
                .efforts
                .contains(&aether_support::ReasoningEffort::None)
        );
        assert_eq!(options[1].reasoning.as_ref(), Some(&expected));
        assert!(options[2].reasoning.is_none());
        assert!(
            store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT reasoning FROM models WHERE model_id='claude-sonnet-5-5'",
                    [],
                    |row| row.get::<_, Option<String>>(0)
                )
                .unwrap()
                .is_none()
        );
        let override_json = r#"{"dialect":"anthropic-adaptive","efforts":["high"]}"#;
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE models SET reasoning=?1 WHERE model_id='claude-sonnet-5-5'",
                [override_json],
            )
            .unwrap();
        drop(store);
        let restarted = Store::open(dir.path()).unwrap();
        assert_eq!(
            restarted
                .resolve(&reference)
                .unwrap()
                .reasoning
                .unwrap()
                .efforts,
            vec![aether_support::ReasoningEffort::High]
        );
    }

    #[test]
    fn version_two_upgrade_preserves_models_credentials_and_native_declarations() {
        for already_has_native in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("providers.db");
            let db = Db::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, key_ciphertext BLOB, position INTEGER NOT NULL, UNIQUE(provider_id, slug));
                 CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, PRIMARY KEY(endpoint_id, model_id));
                 INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1);
                 INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-responses', X'01020304', 0);
                 INSERT INTO models VALUES ('e', 'saved-model', 'chat', 'user', 200000, 8192, 1, 1, 0, '{\"dialect\":\"openai\",\"efforts\":[\"low\",\"high\"]}');
                 PRAGMA user_version = 2;",
            ).unwrap();
            if already_has_native {
                db.execute_batch("ALTER TABLE models ADD COLUMN native_web_search INTEGER NOT NULL DEFAULT 0; UPDATE models SET native_web_search = 1;").unwrap();
            }
            drop(db);
            for _ in 0..2 {
                let store = Store::open(dir.path()).unwrap();
                let options = store.model_options(ModelFilter::default()).unwrap();
                assert_eq!(options[0].model_id, "saved-model");
                assert_eq!(options[0].native_web_search, already_has_native);
                assert_eq!(options[0].reasoning.as_ref().unwrap().efforts.len(), 2);
                assert_eq!(
                    store.list().unwrap()[0].endpoints[0].models[0].context_window,
                    Some(200_000)
                );
                let db = store.lock().unwrap();
                let cipher: Vec<u8> = db
                    .query_row(
                        "SELECT key_ciphertext FROM endpoints WHERE id = 'e'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(cipher, [1, 2, 3, 4]);
                assert_eq!(
                    db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                        .unwrap(),
                    4
                );
            }
            assert!(path.with_file_name("providers.db.bak-v2").is_file());
        }
    }

    #[test]
    fn disabled_provider_leaves_pickers_and_cannot_resolve_until_re_enabled() {
        let (_dir, store) = store();
        let saved = store
            .save(request(
                json!({"name":"Relay","endpoints":[endpoint(None, "a", &["m1"])]}),
            ))
            .unwrap();
        let (id, endpoint_id) = (saved[0].id.clone(), saved[0].endpoints[0].id.clone());
        assert!(saved[0].enabled);
        let reference = ModelRef {
            endpoint_id: endpoint_id.clone(),
            model_id: "m1".to_owned(),
        };
        let off = json!({"id": id, "name":"Relay","enabled":false,"endpoints":[endpoint(Some(&endpoint_id), "a", &["m1"])]});
        assert!(!store.save(request(off)).unwrap()[0].enabled);
        assert!(
            store
                .model_options(ModelFilter::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.resolve(&reference).unwrap_err().code(),
            KitError::not_found("model").code()
        );
        // Omitting `enabled` keeps it off; only an explicit `true` turns it back on.
        let keep = json!({"id": id, "name":"Relay 2","endpoints":[endpoint(Some(&endpoint_id), "a", &["m1"])]});
        assert!(!store.save(request(keep)).unwrap()[0].enabled);
        let on = json!({"id": id, "name":"Relay","enabled":true,"endpoints":[endpoint(Some(&endpoint_id), "a", &["m1"])]});
        assert!(store.save(request(on)).unwrap()[0].enabled);
        assert_eq!(
            store.model_options(ModelFilter::default()).unwrap().len(),
            1
        );
        assert!(store.resolve(&reference).is_ok());
    }

    #[test]
    fn native_search_declaration_survives_reopen_and_rejects_chat_protocol() {
        let (dir, store) = store();
        let mut e = endpoint(None, "a", &["m1"]);
        e["models"][0]["nativeWebSearch"] = json!(true);
        assert!(
            store
                .save(request(json!({"name":"Native","endpoints":[e.clone()]})))
                .is_err()
        );
        e["protocol"] = json!("openai-responses");
        let saved = store
            .save(request(json!({"name":"Native","endpoints":[e]})))
            .unwrap();
        let reference = ModelRef {
            endpoint_id: saved[0].endpoints[0].id.clone(),
            model_id: "m1".into(),
        };
        drop(store);
        let reopened = Store::open(dir.path()).unwrap();
        assert!(reopened.resolve(&reference).unwrap().native_web_search);
        assert!(reopened.model_options(ModelFilter::default()).unwrap()[0].native_web_search);
    }
    #[test]
    fn save_replaces_the_whole_provider_and_survives_reopen() {
        let (dir, store) = store();
        let saved = store
            .save(request(json!({ "name": "Acme", "endpoints": [
                endpoint(None, "a", &["m1", "m2"]), endpoint(None, "b", &["m3"]) ] })))
            .unwrap();
        let provider = &saved[0];
        let (a, b) = (&provider.endpoints[0], &provider.endpoints[1]);
        assert_eq!(a.probe_url, "https://a.example.com/v1/models");
        assert_eq!(a.models[0].context_window, Some(200_000));

        // Keep `a` (renamed to b's slug, models trimmed and reordered), drop `b`, add `c`.
        let saved = store
            .save(request(
                json!({ "id": provider.id, "name": "Acme 2", "endpoints": [
                endpoint(None, "a", &["n1"]), endpoint(Some(&a.id), "b", &["m2", "x"]) ] }),
            ))
            .unwrap();
        let endpoints = &saved[0].endpoints;
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Acme 2");
        assert_eq!(endpoints[1].id, a.id, "id carried back keeps the endpoint");
        assert_eq!(endpoints[1].slug, "b");
        let ids: Vec<_> = endpoints[1].models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m2", "x"]);
        assert_ne!(
            endpoints[0].id, b.id,
            "slug match alone does not keep an endpoint"
        );
        let gone = ModelRef {
            endpoint_id: b.id.clone(),
            model_id: "m3".to_owned(),
        };
        assert_eq!(
            store.resolve(&gone).err().unwrap().details().entity(),
            Some("model")
        );

        // Unknown endpoint id: rejected, nothing written.
        let error = store
            .save(request(json!({ "id": provider.id, "name": "X",
                "endpoints": [endpoint(Some(&b.id), "z", &[])] })))
            .err()
            .unwrap();
        assert_eq!(error.code(), KitErrorCode::NotFound);
        drop(store);
        assert_eq!(Store::open(dir.path()).unwrap().list().unwrap(), saved);
    }
    #[test]
    fn set_enabled_changes_only_provider_switch() {
        let (_dir, store) = store();
        let saved = store
            .save(request(json!({ "name": "Acme", "endpoints": [endpoint(None, "main", &["m1"])] })))
            .unwrap();
        let provider = &saved[0];
        let endpoint_id = provider.endpoints[0].id.clone();
        let changed = store.set_enabled(&provider.id, false).unwrap();
        assert!(!changed[0].enabled);
        assert_eq!(changed[0].name, "Acme");
        assert_eq!(changed[0].endpoints[0].id, endpoint_id);
        assert_eq!(changed[0].endpoints[0].models[0].id, "m1");
    }

    #[test]
    fn key_is_tri_state() {
        let (_dir, store) = store();
        let mut first = endpoint(None, "a", &["m1"]);
        first["apiKey"] = json!(KEY);
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [first] })))
            .unwrap();
        let (provider_id, ep) = (saved[0].id.clone(), saved[0].endpoints[0].clone());
        assert!(ep.has_key && ep.key_readable);
        assert_eq!(ep.key_masked.as_deref(), Some("sk-t**************abcd"));
        let resolved = store
            .resolve(&ModelRef {
                endpoint_id: ep.id.clone(),
                model_id: "m1".to_owned(),
            })
            .unwrap();
        assert_eq!(resolved.api_key.unwrap().as_str(), KEY);
        assert_eq!(resolved.base_url, "https://a.example.com/v1");

        // Field omitted: keep.
        let keep = endpoint(Some(&ep.id), "a", &["m1"]);
        store
            .save(request(
                json!({ "id": provider_id, "name": "P", "endpoints": [keep] }),
            ))
            .unwrap();
        assert_eq!(key_of(&store, &ep.id).as_deref(), Some(KEY));

        // "": clear.
        let mut clear = endpoint(Some(&ep.id), "a", &["m1"]);
        clear["apiKey"] = json!("");
        let saved = store
            .save(request(
                json!({ "id": provider_id, "name": "P", "endpoints": [clear] }),
            ))
            .unwrap();
        let ep = &saved[0].endpoints[0];
        assert!(!ep.has_key && ep.key_readable && ep.key_masked.is_none());
        assert_eq!(key_of(&store, &ep.id), None);
    }

    #[test]
    fn seal_failure_leaves_everything_unchanged() {
        let (_dir, store) = store();
        let mut first = endpoint(None, "a", &["m1"]);
        first["apiKey"] = json!(KEY);
        let before = store
            .save(request(json!({ "name": "P", "endpoints": [first] })))
            .unwrap();
        let ep_id = before[0].endpoints[0].id.clone();
        let mut change = endpoint(Some(&ep_id), "renamed", &[]);
        change["apiKey"] = json!("sk-new-key-0000000000");
        let input = request(json!({ "id": before[0].id, "name": "Q",
            "endpoints": [change, endpoint(None, "extra", &[])] }));
        let error = store.save_with(input, fail_seal).err().unwrap();
        assert_eq!(error.code(), KitErrorCode::Unsupported);
        assert_eq!(store.list().unwrap(), before);
        assert_eq!(key_of(&store, &ep_id).as_deref(), Some(KEY));
    }

    #[test]
    fn unreadable_ciphertext_is_reported_not_fatal() {
        let (_dir, store) = store();
        let saved = store
            .save(request(
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &["m1"])] }),
            ))
            .unwrap();
        let ep_id = saved[0].endpoints[0].id.clone();
        // Unknown format marker, as after copying the database to another platform.
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE endpoints SET key_ciphertext = X'7F0102' WHERE id = ?1",
                [&ep_id],
            )
            .unwrap();
        let ep = &store.list().unwrap()[0].endpoints[0];
        assert!(ep.has_key && !ep.key_readable && ep.key_masked.is_none());
        let error = store.endpoint_connection(&ep_id).err().unwrap();
        assert_eq!(error.code(), KitErrorCode::NeedsConfig);
        assert_eq!(error.details().reason(), Some("key_unreadable"));
    }

    #[test]
    fn delete_and_model_options() {
        let (_dir, store) = store();
        let mut ep = endpoint(None, "a", &["gpt", "embed"]);
        ep["models"][1]["modelType"] = json!("embedding");
        ep["models"][1]["supportsTools"] = json!(null);
        let saved = store
            .save(request(json!({ "name": "Acme", "endpoints": [ep] })))
            .unwrap();
        let options = store
            .model_options(ModelFilter {
                model_type: None,
                supports_tools: Some(true),
            })
            .unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].label, "Acme · a · gpt");
        let embeddings = ModelFilter {
            model_type: Some(ModelType::Embedding),
            supports_tools: None,
        };
        assert_eq!(
            store.model_options(embeddings).unwrap()[0].model_id,
            "embed"
        );

        assert!(store.delete(&saved[0].id).unwrap().is_empty());
        assert!(
            store
                .model_options(ModelFilter::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.delete(&saved[0].id).err().unwrap().code(),
            KitErrorCode::NotFound
        );
    }

    #[test]
    fn invalid_input_is_rejected() {
        let (_dir, store) = store();
        let cases = [
            (json!({ "name": " ", "endpoints": [] }), "name"),
            (
                json!({ "name": "P", "endpoints": [endpoint(None, "Bad_Slug", &[])] }),
                "slug",
            ),
            (
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &[]), endpoint(None, "a", &[])] }),
                "slug",
            ),
            (
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &["m", "m"])] }),
                "models",
            ),
        ];
        for (input, field) in cases {
            let error = store.save(request(input)).err().unwrap();
            assert_eq!(
                (error.code(), error.details().field()),
                (KitErrorCode::BadRequest, Some(field))
            );
        }
        let mut ftp = endpoint(None, "a", &[]);
        ftp["baseUrl"] = json!("ftp://x");
        let error = store
            .save(request(json!({ "name": "P", "endpoints": [ftp] })))
            .err()
            .unwrap();
        assert_eq!(error.details().field(), Some("baseUrl"));
        assert!(store.list().unwrap().is_empty());
        let unknown = json!({ "name": "P", "endpoints": [], "roles": [] });
        assert!(serde_json::from_value::<ProviderSave>(unknown).is_err());
    }
}
