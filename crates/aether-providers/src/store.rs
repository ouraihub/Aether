//! `providers.db`: whole-provider save, per-endpoint keys and connection snapshots (docs/11 §2–§4, docs/34).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use aether_support::clock::now_millis;

use aether_support::db::{self, Migration};
use aether_support::id::new_id;
use aether_support::mask::mask_secret;
use aether_support::secret;
use aether_support::{Connection, KitError, ModelPricing, Protocol, ReasoningConfig};
use rusqlite::{Connection as Db, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::connection::{self, ConnectionSnapshot, EndpointSnapshot};
use crate::dto::{
    Endpoint, EndpointKey, EndpointKeySave, EndpointSave, KeyStrategy, Model, ModelFilter,
    ModelOption, ModelRef, ModelType, ProbeResult, ProbeTarget, Provider, ProviderSave, TypeSource,
};
use crate::probe::{self, ProbeInput};
use crate::{api_base, infer_reasoning};
use tokio_util::sync::CancellationToken;

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
const MAX_NOTES_CHARS: usize = 10_000;
const MAX_ENDPOINTS: usize = 20;
const MAX_MODELS: usize = 1000;
const MAX_SLUG: usize = 40;
const MAX_BASE_URL: usize = 2048;
const MAX_KEYS: i64 = 16;
const MAX_LABEL_CHARS: usize = 40;
const MAX_KEY_BYTES: usize = 4096;

/// Provider configuration stored in `<root>/providers.db`.
pub struct Store {
    db: Mutex<Db>,
    /// Next rotation start per endpoint id; in memory, so a restart begins at the first key.
    cursors: Mutex<HashMap<String, usize>>,
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
            // Old single keys are dropped, not migrated (docs/34 §4).
            Migration {
                version: 5,
                sql: "ALTER TABLE endpoints DROP COLUMN key_ciphertext;
CREATE TABLE endpoint_keys (
  id TEXT PRIMARY KEY,
  endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
  label TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1,
  key_ciphertext BLOB NOT NULL, position INTEGER NOT NULL
);
CREATE INDEX idx_endpoint_keys ON endpoint_keys(endpoint_id, position, id);",
            },
            Migration {
                version: 6,
                sql: "ALTER TABLE endpoints ADD COLUMN image_base_url TEXT;
ALTER TABLE endpoints ADD COLUMN headers TEXT NOT NULL DEFAULT '{}';",
            },
            Migration {
                version: 7,
                sql: "ALTER TABLE endpoints ADD COLUMN image_edit_base_url TEXT;",
            },
            Migration {
                version: 8,
                sql: "ALTER TABLE providers ADD COLUMN notes TEXT NOT NULL DEFAULT '';",
            },
            Migration {
                version: 9,
                sql: "ALTER TABLE providers ADD COLUMN preset_id TEXT;
CREATE UNIQUE INDEX idx_providers_preset ON providers(preset_id) WHERE preset_id IS NOT NULL;",
            },
            Migration {
                version: 10,
                sql: "ALTER TABLE models ADD COLUMN name TEXT;
ALTER TABLE models ADD COLUMN group_name TEXT;
ALTER TABLE models ADD COLUMN pricing TEXT;",
            },
        ];
        let db = db::open(&path, &steps)?;
        Ok(Self {
            db: Mutex::new(db),
            cursors: Mutex::default(),
        })
    }

    /// All providers with endpoints and models in display order.
    ///
    /// # Errors
    /// `internal` when SQLite fails or stored data is malformed.
    pub fn list(&self) -> Result<Vec<Provider>, KitError> {
        list(&*self.lock()?)
    }

    /// Turns a provider on or off without touching anything else.
    ///
    /// # Errors
    /// `not_found` for an unknown id; `internal` when SQLite fails.
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

    /// Replaces one provider as a whole (docs/11 §3): endpoints missing from the request
    /// are deleted with their models and keys. Keys themselves change only via [`Self::save_key`].
    ///
    /// # Errors
    /// `bad_request` for invalid input; `not_found` for an unknown provider or endpoint id.
    pub fn save(&self, input: ProviderSave) -> Result<Vec<Provider>, KitError> {
        validate(&input)?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let now = now_millis();
        let provider_id = if let Some(id) = &input.id {
            let sql = "UPDATE providers SET name = ?2, updated_at = ?3, enabled = COALESCE(?4, enabled), notes = COALESCE(?5, notes) WHERE id = ?1";
            if tx.execute(
                sql,
                params![id, input.name, now, input.enabled, input.notes],
            )? == 0
            {
                return Err(KitError::not_found("provider"));
            }
            id.clone()
        } else {
            if let Some(preset) = &input.preset_id {
                // One provider per preset; a same-named user provider adopts it (docs/39 §3).
                let seeded: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM providers WHERE preset_id = ?1)",
                    [preset],
                    |row| row.get(0),
                )?;
                if !seeded {
                    tx.execute(
                        "UPDATE providers SET preset_id = ?1 WHERE rowid = (SELECT rowid FROM providers WHERE preset_id IS NULL AND lower(name) = lower(?2) ORDER BY position LIMIT 1)",
                        params![preset, input.name.trim()],
                    )?;
                }
                let adopted: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM providers WHERE preset_id = ?1)",
                    [preset],
                    |row| row.get(0),
                )?;
                if seeded || adopted {
                    tx.commit()?;
                    return list(&db);
                }
            }
            let id = new_id()?;
            tx.execute(
                "INSERT INTO providers (id, name, position, created_at, updated_at, enabled, notes, preset_id)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(position) + 1, 0) FROM providers), ?3, ?3, ?4, ?5, ?6)",
                params![id, input.name, now, input.enabled.unwrap_or(true), input.notes.as_deref().unwrap_or(""), input.preset_id],
            )?;
            id
        };

        let old: HashSet<String> = tx
            .prepare("SELECT id FROM endpoints WHERE provider_id = ?1")?
            .query_map([&provider_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let kept: HashSet<&str> = input
            .endpoints
            .iter()
            .filter_map(|e| e.id.as_deref())
            .collect();
        if kept.iter().any(|id| !old.contains(*id)) {
            return Err(KitError::not_found("endpoint"));
        }
        // Ciphertexts to `forget` once the new state is committed (docs/10 §2).
        let mut stale = Vec::new();
        for id in old.iter().filter(|id| !kept.contains(id.as_str())) {
            stale.extend(ciphertexts(&tx, "endpoint_id = ?1", [id])?);
            tx.execute("DELETE FROM endpoints WHERE id = ?1", [id])?;
        }
        // Kept endpoints may swap slugs; park them on their unique ids first so the
        // UNIQUE (provider_id, slug) check never sees a transient duplicate.
        tx.execute(
            "UPDATE endpoints SET slug = id WHERE provider_id = ?1",
            [&provider_id],
        )?;
        for (position, endpoint) in input.endpoints.into_iter().enumerate() {
            write_endpoint(&tx, &provider_id, position, &endpoint)?;
        }
        tx.commit()?;
        forget_all(&stale)?;
        list(&db)
    }

    /// Adds (no `id`) or patches (with `id`) one key of an endpoint (docs/34 §3.1–§3.2).
    ///
    /// # Errors
    /// `not_found{endpoint|key}`; `bad_request{apiKey: empty|too_long|duplicate}`,
    /// `bad_request{label: too_long}`, `bad_request{keys: too_many}`; the sealing error
    /// (e.g. `needs_config{secret_store_unavailable}`), in which case nothing is written.
    pub fn save_key(&self, input: EndpointKeySave) -> Result<Vec<Provider>, KitError> {
        let had_key = input.key.is_some();
        // The request's plaintext is wiped on drop like every other clear key.
        let plain = input.key.map(Zeroizing::new);
        let key = plain.as_deref().map(|key| key.trim());
        if key.is_some_and(|key| key.len() > MAX_KEY_BYTES) {
            return Err(KitError::bad_request("apiKey", "too_long"));
        }
        let key = key.filter(|key| !key.is_empty());
        if key.is_none() && (input.id.is_none() || had_key) {
            return Err(KitError::bad_request("apiKey", "empty"));
        }
        let label = input.label.as_deref().map(str::trim);
        if label.is_some_and(|label| label.chars().count() > MAX_LABEL_CHARS) {
            return Err(KitError::bad_request("label", "too_long"));
        }
        // Sealed before any write: a failing seal leaves everything unchanged.
        let sealed = key.map(secret::seal).transpose()?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let endpoint_known = tx
            .query_row(
                "SELECT 1 FROM endpoints WHERE id = ?1",
                [&input.endpoint_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !endpoint_known {
            return Err(KitError::not_found("endpoint"));
        }
        if let Some(key) = key {
            let others = tx
                .prepare("SELECT key_ciphertext FROM endpoint_keys WHERE endpoint_id = ?1 AND id IS NOT ?2")?
                .query_map(params![input.endpoint_id, input.id], |row| row.get::<_, Vec<u8>>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            // Unreadable keys cannot be compared; they are not duplicates of anything typed.
            if others
                .iter()
                .any(|cipher| secret::open(cipher).is_ok_and(|plain| plain.as_str() == key))
            {
                return Err(KitError::bad_request("apiKey", "duplicate"));
            }
        }
        let mut stale = Vec::new();
        if let Some(id) = &input.id {
            let old: Vec<u8> = tx
                .query_row(
                    "SELECT key_ciphertext FROM endpoint_keys WHERE id = ?1 AND endpoint_id = ?2",
                    params![id, input.endpoint_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| KitError::not_found("key"))?;
            if sealed.is_some() {
                stale.push(old);
            }
            tx.execute(
                "UPDATE endpoint_keys SET label = COALESCE(?2, label), enabled = COALESCE(?3, enabled),
                 key_ciphertext = COALESCE(?4, key_ciphertext) WHERE id = ?1",
                params![id, label, input.enabled, sealed],
            )?;
        } else {
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM endpoint_keys WHERE endpoint_id = ?1",
                [&input.endpoint_id],
                |row| row.get(0),
            )?;
            if count >= MAX_KEYS {
                return Err(KitError::bad_request("keys", "too_many"));
            }
            tx.execute(
                "INSERT INTO endpoint_keys (id, endpoint_id, label, enabled, key_ciphertext, position)
                 SELECT ?1, ?2, ?3, ?4, ?5, COALESCE(MAX(position) + 1, 0) FROM endpoint_keys WHERE endpoint_id = ?2",
                params![
                    new_id()?,
                    input.endpoint_id,
                    label.unwrap_or_default(),
                    input.enabled.unwrap_or(true),
                    sealed
                ],
            )?;
        }
        tx.commit()?;
        forget_all(&stale)?;
        list(&db)
    }

    /// Deletes one key of an endpoint.
    ///
    /// # Errors
    /// `not_found{key}` when the key does not exist or belongs to another endpoint.
    pub fn remove_key(&self, endpoint_id: &str, key_id: &str) -> Result<Vec<Provider>, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let stale = ciphertexts(&tx, "id = ?1 AND endpoint_id = ?2", [key_id, endpoint_id])?;
        if stale.is_empty() {
            return Err(KitError::not_found("key"));
        }
        tx.execute("DELETE FROM endpoint_keys WHERE id = ?1", [key_id])?;
        tx.commit()?;
        forget_all(&stale)?;
        list(&db)
    }

    /// The plaintext of one stored key, on explicit user request.
    ///
    /// # Errors
    /// `not_found{key}`; `needs_config{reason:"key_unreadable"}`.
    pub fn reveal_key(&self, key_id: &str) -> Result<Zeroizing<String>, KitError> {
        let cipher: Vec<u8> = self
            .lock()?
            .query_row(
                "SELECT key_ciphertext FROM endpoint_keys WHERE id = ?1",
                [key_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("key"))?;
        secret::open(&cipher)
    }

    /// Deletes a provider with its endpoints and models.
    ///
    /// # Errors
    /// `not_found` for an unknown id; `internal` when SQLite fails.
    pub fn delete(&self, id: &str) -> Result<Vec<Provider>, KitError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        // The cascade does not call `secret::forget`, so collect ciphertexts first (docs/10 §2).
        let stale = ciphertexts(
            &tx,
            "endpoint_id IN (SELECT id FROM endpoints WHERE provider_id = ?1)",
            [id],
        )?;
        let preset: Option<Option<String>> = tx
            .query_row(
                "SELECT preset_id FROM providers WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match preset {
            None => return Err(KitError::not_found("provider")),
            Some(Some(_)) => return Err(KitError::conflict("preset_provider")),
            Some(None) => {}
        }
        tx.execute("DELETE FROM providers WHERE id = ?1", [id])?;
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
            "SELECT p.name, e.slug, m.endpoint_id, m.model_id, m.model_type, m.max_output, m.supports_tools, m.reasoning, m.native_web_search, e.protocol, m.can_vision, m.name, e.base_url
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
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
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
            name,
            base_url,
        ) in rows
        {
            let model_type = ModelType::parse(&model_type).ok_or_else(|| corrupt("model_type"))?;
            let protocol = parse_protocol(&protocol)?;
            let reasoning = read_reasoning(
                reasoning.as_deref(),
                &model_id,
                model_type,
                protocol,
                &base_url,
                max_output,
            )?;
            if filter.model_type.is_some_and(|wanted| wanted != model_type)
                || filter
                    .supports_tools
                    .is_some_and(|wanted| supports_tools != Some(wanted))
            {
                continue;
            }
            options.push(ModelOption {
                label: format!("{provider} · {slug} · {model_id}"),
                name,
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

    /// Reads the raw endpoint and effective model metadata, reserving one fixed key order.
    /// Reasoning is resolved here, including an explicit user disable.
    ///
    /// # Errors
    /// `not_found{entity:"model"}`; `needs_config{reason:"missing_url"}` for an empty address;
    /// `needs_config{reason:"key_unreadable"}` when an enabled key cannot be decrypted;
    /// `internal` when SQLite fails.
    pub fn snapshot(&self, model: &ModelRef) -> Result<ConnectionSnapshot, KitError> {
        let db = self.lock()?;
        let row = db.query_row(
            "SELECT m.model_type, m.context_window, m.max_output, m.supports_tools, m.reasoning, m.native_web_search, m.can_vision, m.pricing, m.type_source, m.name, m.group_name
             FROM models m JOIN endpoints e ON e.id = m.endpoint_id JOIN providers p ON p.id = e.provider_id
             WHERE m.endpoint_id = ?1 AND m.model_id = ?2 AND p.enabled = 1",
            params![model.endpoint_id, model.model_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<u32>>(1)?, row.get::<_, Option<u32>>(2)?, row.get::<_, Option<bool>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, bool>(5)?, row.get::<_, bool>(6)?, row.get::<_, Option<String>>(7)?, row.get::<_, String>(8)?, row.get::<_, Option<String>>(9)?, row.get::<_, Option<String>>(10)?)),
        ).optional()?;
        let (
            model_type,
            context_window,
            max_output,
            supports_tools,
            reasoning,
            native_web_search,
            can_vision,
            pricing,
            type_source,
            name,
            group_name,
        ) = row.ok_or_else(|| KitError::not_found("model"))?;
        let mut row = self.read_endpoint(&db, &model.endpoint_id)?;
        if row.base_url.trim().is_empty() {
            return Err(KitError::needs_config("missing_url"));
        }
        let protocol = row.protocol;
        let model_type = ModelType::parse(&model_type).ok_or_else(|| corrupt("model_type"))?;
        let reasoning = read_reasoning(
            reasoning.as_deref(),
            &model.model_id,
            model_type,
            protocol,
            &row.base_url,
            max_output,
        )?;
        let pricing = read_pricing(pricing.as_deref())?;
        connection::validate_endpoint(&row)?;
        let metadata = Model {
            id: model.model_id.clone(),
            model_type,
            type_source: TypeSource::parse(&type_source).ok_or_else(|| corrupt("type_source"))?,
            context_window,
            max_output,
            supports_tools,
            reasoning,
            native_web_search,
            can_vision,
            pricing,
            name,
            group_name,
        };
        connection::validate_model(&row, &metadata)?;
        row.api_keys = self.keys(&db, &model.endpoint_id)?;
        Ok(ConnectionSnapshot {
            endpoint: row,
            model: Some(metadata),
        })
    }

    /// Reads a model snapshot once, then prepares its connection without further rotation.
    /// # Errors
    /// Reports invalid model configuration, missing models, unreadable keys or storage errors.
    pub fn resolve(&self, model: &ModelRef) -> Result<Connection, KitError> {
        connection::prepare(self.snapshot(model)?)
    }

    /// Reads one raw stored endpoint and reserves its key order once.
    /// # Errors
    /// Reports missing endpoints, unreadable credentials or storage errors.
    pub fn endpoint_snapshot(&self, endpoint_id: &str) -> Result<EndpointSnapshot, KitError> {
        self.endpoint_row(&*self.lock()?, endpoint_id)
    }

    /// Prepares an endpoint without requiring a selected model.
    /// # Errors
    /// Reports invalid configuration, missing endpoints, unreadable keys or storage errors.
    pub fn endpoint_connection(&self, endpoint_id: &str) -> Result<Connection, KitError> {
        connection::prepare(ConnectionSnapshot {
            endpoint: self.endpoint_snapshot(endpoint_id)?,
            model: None,
        })
    }

    /// Probes saved configuration or a draft through the independent probe capability.
    /// # Errors
    /// Reports invalid input, missing endpoints/keys, unreadable credentials or cancellation.
    pub async fn probe(
        &self,
        target: ProbeTarget,
        cancel: &CancellationToken,
    ) -> Result<ProbeResult, KitError> {
        let input = {
            let db = self.lock()?;
            let endpoint = match target {
                ProbeTarget::Saved { endpoint_id } => self.endpoint_row(&db, &endpoint_id)?,
                ProbeTarget::Draft {
                    base_url,
                    protocol,
                    endpoint_id,
                    key_id,
                } => {
                    connection::validate_url("url", &base_url)?;
                    match (endpoint_id, key_id) {
                        (Some(endpoint_id), key_id) => {
                            let mut endpoint = self.read_endpoint(&db, &endpoint_id)?;
                            endpoint.base_url = base_url;
                            endpoint.protocol = protocol;
                            connection::validate_endpoint(&endpoint)?;
                            endpoint.api_keys = if let Some(key_id) = key_id {
                                vec![self.endpoint_key(&db, &endpoint_id, &key_id)?]
                            } else {
                                self.keys(&db, &endpoint_id)?
                            };
                            endpoint
                        }
                        (None, Some(_)) => return Err(KitError::not_found("key")),
                        (None, None) => EndpointSnapshot {
                            endpoint_id: None,
                            base_url,
                            protocol,
                            api_keys: Vec::new(),
                            headers: Vec::new(),
                            image_base_url: None,
                            image_edit_base_url: None,
                        },
                    }
                }
            };
            ProbeInput {
                base_url: endpoint.base_url,
                protocol: endpoint.protocol,
                api_keys: endpoint.api_keys,
                headers: endpoint.headers,
                policy: aether_support::net::AddressPolicy::default(),
            }
        };
        probe::probe(input, cancel).await
    }

    fn read_endpoint(&self, db: &Db, endpoint_id: &str) -> Result<EndpointSnapshot, KitError> {
        let row = db
            .query_row(
                "SELECT base_url, protocol, headers, image_base_url, image_edit_base_url FROM endpoints WHERE id = ?1",
                [endpoint_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?;
        let (base_url, protocol, headers, image_base_url, image_edit_base_url) =
            row.ok_or_else(|| KitError::not_found("endpoint"))?;
        Ok(EndpointSnapshot {
            endpoint_id: Some(endpoint_id.to_owned()),
            protocol: parse_protocol(&protocol)?,
            api_keys: Vec::new(),
            headers: parse_headers(&headers)?.into_iter().collect(),
            image_base_url,
            image_edit_base_url,
            base_url,
        })
    }

    fn endpoint_row(&self, db: &Db, endpoint_id: &str) -> Result<EndpointSnapshot, KitError> {
        let mut endpoint = self.read_endpoint(db, endpoint_id)?;
        connection::validate_endpoint(&endpoint)?;
        endpoint.api_keys = self.keys(db, endpoint_id)?;
        Ok(endpoint)
    }

    /// One stored key of `endpoint_id`, for a probe of that key alone.
    fn endpoint_key(
        &self,
        db: &Db,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Zeroizing<String>, KitError> {
        let cipher: Vec<u8> = db
            .query_row(
                "SELECT key_ciphertext FROM endpoint_keys WHERE id = ?1 AND endpoint_id = ?2",
                [key_id, endpoint_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| KitError::not_found("key"))?;
        secret::open(&cipher)
    }

    /// Enabled keys of an endpoint, decrypted, starting at this endpoint's cursor and
    /// wrapping once; advances the cursor (docs/34 §3.4). Empty = no key.
    fn keys(&self, db: &Db, endpoint_id: &str) -> Result<Vec<Zeroizing<String>>, KitError> {
        let mut keys = db
            .prepare_cached("SELECT key_ciphertext FROM endpoint_keys WHERE endpoint_id = ?1 AND enabled = 1 ORDER BY position, id")?
            .query_map([endpoint_id], |row| row.get::<_, Vec<u8>>(0))?
            .map(|cipher| secret::open(&cipher?))
            .collect::<Result<Vec<_>, _>>()?;
        if !keys.is_empty() {
            let mut cursors = self.cursors.lock().map_err(|_| {
                KitError::internal(std::io::Error::other("providers cursor lock poisoned"))
            })?;
            let cursor = cursors.entry(endpoint_id.to_owned()).or_default();
            let start = *cursor % keys.len();
            *cursor = (start + 1) % keys.len();
            keys.rotate_left(start);
        }
        Ok(keys)
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
    if input
        .notes
        .as_ref()
        .is_some_and(|notes| notes.chars().count() > MAX_NOTES_CHARS)
    {
        return Err(too_long("notes", MAX_NOTES_CHARS));
    }
    if input
        .preset_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 64)
    {
        return Err(too_long("presetId", 64));
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
        // Empty is allowed: the endpoint exists before its address (docs/34 §3.5).
        if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
            return Err(KitError::bad_request("baseUrl", "invalid_url"));
        }
        for (field, image) in [
            ("imageBaseUrl", &endpoint.image_base_url),
            ("imageEditBaseUrl", &endpoint.image_edit_base_url),
        ] {
            if let Some(Some(image)) = image {
                let image = image.trim();
                if !image.is_empty()
                    && (image.len() > MAX_BASE_URL
                        || !(image.starts_with("http://") || image.starts_with("https://")))
                {
                    return Err(KitError::bad_request(field, "invalid_url"));
                }
            }
        }
        if let Some(headers) = &endpoint.headers {
            validate_headers(headers)?;
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
            label("name", model.name.as_deref())?;
            label("groupName", model.group_name.as_deref())?;
            if model.context_window.is_some_and(|n| n < 1) {
                return Err(KitError::bad_request("contextWindow", "invalid"));
            }
            if model.max_output.is_some_and(|n| n < 1) {
                return Err(KitError::bad_request("maxOutput", "invalid"));
            }
            if model.model_type == ModelType::Chat
                && let (Some(window), Some(output)) = (model.context_window, model.max_output)
                && output >= window
            {
                return Err(KitError::bad_request("maxOutput", "exceeds_context"));
            }
            if model
                .pricing
                .as_ref()
                .is_some_and(|p| model.model_type != ModelType::Chat || !p.is_valid())
            {
                return Err(KitError::bad_request("pricing", "invalid"));
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

/// Stored `models.reasoning` of a model the user turned reasoning off for.
const REASONING_OFF: &str = "none";

/// The one read path of `models.reasoning`: NULL = not set (inferred for chat models),
/// `'none'` = turned off, otherwise the stored config (docs/41 §2).
fn read_reasoning(
    stored: Option<&str>,
    model_id: &str,
    model_type: ModelType,
    protocol: Protocol,
    base_url: &str,
    max_output: Option<u32>,
) -> Result<Option<ReasoningConfig>, KitError> {
    match stored {
        Some(REASONING_OFF) => Ok(None),
        Some(json) => serde_json::from_str(json)
            .map(Some)
            .map_err(KitError::internal),
        None => Ok((model_type == ModelType::Chat)
            .then(|| infer_reasoning(model_id, protocol, base_url, max_output))
            .flatten()),
    }
}

fn read_pricing(stored: Option<&str>) -> Result<Option<ModelPricing>, KitError> {
    stored
        .map(serde_json::from_str)
        .transpose()
        .map_err(KitError::internal)
}

/// Trimmed label; blank = `None`.
fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn label(field: &str, value: Option<&str>) -> Result<(), KitError> {
    if trimmed(value).is_some_and(|v| v.chars().count() > MAX_NAME_CHARS) {
        return Err(KitError::bad_request(field, "too_long")
            .with_limit(u64::try_from(MAX_NAME_CHARS).unwrap_or(u64::MAX)));
    }
    Ok(())
}

fn too_long(field: &str, limit: usize) -> KitError {
    KitError::bad_request(field, "out_of_range")
        .with_limit(u64::try_from(limit).unwrap_or(u64::MAX))
}

fn validate_headers(headers: &BTreeMap<String, String>) -> Result<(), KitError> {
    connection::validate_headers(
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    )
}

fn write_endpoint(
    tx: &Db,
    provider_id: &str,
    position: usize,
    endpoint: &EndpointSave,
) -> Result<(), KitError> {
    let position = i64::try_from(position).unwrap_or(i64::MAX);
    let base_url = endpoint.base_url.trim();
    let protocol = protocol_str(endpoint.protocol);
    // `None` = keep the stored value (new endpoints: null / `{}`).
    let trim = |image: &Option<Option<String>>| {
        image.as_ref().map(|image| {
            image
                .as_deref()
                .map(str::trim)
                .filter(|image| !image.is_empty())
                .map(str::to_owned)
        })
    };
    let image = trim(&endpoint.image_base_url);
    let edit = trim(&endpoint.image_edit_base_url);
    let headers = endpoint
        .headers
        .as_ref()
        .map(|headers| {
            let trimmed: BTreeMap<&str, &str> = headers
                .iter()
                .map(|(name, value)| (name.trim(), value.as_str()))
                .collect();
            serde_json::to_string(&trimmed)
        })
        .transpose()
        .map_err(KitError::internal)?;
    let id = if let Some(id) = &endpoint.id {
        tx.execute(
            "UPDATE endpoints SET slug = ?2, base_url = ?3, protocol = ?4, position = ?5,
             image_base_url = CASE WHEN ?6 THEN ?7 ELSE image_base_url END, headers = COALESCE(?8, headers),
             image_edit_base_url = CASE WHEN ?9 THEN ?10 ELSE image_edit_base_url END WHERE id = ?1",
            params![id, endpoint.slug, base_url, protocol, position, image.is_some(), image.flatten(), headers, edit.is_some(), edit.flatten()],
        )?;
        id.clone()
    } else {
        let id = new_id()?;
        tx.execute(
            "INSERT INTO endpoints (id, provider_id, slug, base_url, protocol, position, image_base_url, headers, image_edit_base_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, COALESCE(?8, '{}'), ?9)",
            params![id, provider_id, endpoint.slug, base_url, protocol, position, image.flatten(), headers, edit.flatten()],
        )?;
        id
    };
    let old: HashMap<String, Option<String>> = tx
        .prepare("SELECT model_id, reasoning FROM models WHERE endpoint_id = ?1")?
        .query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    tx.execute("DELETE FROM models WHERE endpoint_id = ?1", [&id])?;
    let mut insert = tx.prepare_cached("INSERT INTO models (endpoint_id, model_id, model_type, type_source, context_window, max_output, can_vision, supports_tools, position, reasoning, native_web_search, name, group_name, pricing) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)")?;
    for (position, model) in endpoint.models.iter().enumerate() {
        // Omitted keeps the stored column, re-validated below; null = off ('none' for chat).
        let reasoning = match &model.reasoning {
            Some(Some(config)) => Some(serde_json::to_string(config).map_err(KitError::internal)?),
            Some(None) => (model.model_type == ModelType::Chat).then(|| REASONING_OFF.to_owned()),
            None => old.get(&model.id).cloned().flatten(),
        };
        if let Some(stored) = reasoning.as_deref().filter(|r| *r != REASONING_OFF) {
            let config: ReasoningConfig =
                serde_json::from_str(stored).map_err(|_| corrupt("reasoning"))?;
            let reason = if model.model_type == ModelType::Chat {
                config.validate(endpoint.protocol, model.max_output).err()
            } else {
                Some("invalid_efforts")
            };
            if let Some(reason) = reason {
                return Err(KitError::bad_request("reasoning", reason));
            }
        }
        let pricing = model
            .pricing
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(KitError::internal)?;
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
            model.native_web_search,
            trimmed(model.name.as_deref()),
            trimmed(model.group_name.as_deref()),
            pricing
        ])?;
    }
    Ok(())
}

fn list(db: &Db) -> Result<Vec<Provider>, KitError> {
    let mut models: HashMap<String, Vec<Model>> = HashMap::new();
    let mut stmt = db.prepare_cached("SELECT m.endpoint_id, m.model_id, m.model_type, m.type_source, m.context_window, m.max_output, m.can_vision, m.supports_tools, m.reasoning, m.native_web_search, e.protocol, m.name, m.group_name, m.pricing, e.base_url FROM models m JOIN endpoints e ON e.id = m.endpoint_id ORDER BY m.endpoint_id, m.position")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let model_type: String = row.get(2)?;
        let type_source: String = row.get(3)?;
        let reasoning: Option<String> = row.get(8)?;
        let protocol = parse_protocol(&row.get::<_, String>(10)?)?;
        let model_type = ModelType::parse(&model_type).ok_or_else(|| corrupt("model_type"))?;
        let max_output = row.get(5)?;
        let model_id: String = row.get(1)?;
        let base_url: String = row.get(14)?;
        models.entry(row.get(0)?).or_default().push(Model {
            id: model_id.clone(),
            model_type,
            type_source: TypeSource::parse(&type_source).ok_or_else(|| corrupt("type_source"))?,
            context_window: row.get(4)?,
            max_output: row.get(5)?,
            can_vision: row.get(6)?,
            supports_tools: row.get(7)?,
            reasoning: read_reasoning(
                reasoning.as_deref(),
                &model_id,
                model_type,
                protocol,
                &base_url,
                max_output,
            )?,
            native_web_search: row.get(9)?,
            name: row.get(11)?,
            group_name: row.get(12)?,
            pricing: read_pricing(row.get::<_, Option<String>>(13)?.as_deref())?,
        });
    }
    let mut keys: HashMap<String, Vec<EndpointKey>> = HashMap::new();
    let mut stmt = db.prepare_cached("SELECT endpoint_id, id, label, enabled, key_ciphertext FROM endpoint_keys ORDER BY endpoint_id, position, id")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let masked = secret::open(&row.get::<_, Vec<u8>>(4)?)
            .ok()
            .map(|plain| mask_secret(&plain));
        keys.entry(row.get(0)?).or_default().push(EndpointKey {
            id: row.get(1)?,
            label: row.get(2)?,
            enabled: row.get(3)?,
            readable: masked.is_some(),
            masked,
            // ponytail: desktop keeps round-robin only and never sets keys aside; persist both when the desktop relay lands.
            exhausted_until: None,
            exhausted_reason: None,
        });
    }
    let mut endpoints: HashMap<String, Vec<Endpoint>> = HashMap::new();
    let mut stmt = db.prepare_cached("SELECT provider_id, id, slug, base_url, protocol, image_base_url, headers, image_edit_base_url FROM endpoints ORDER BY provider_id, position")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(1)?;
        let base_url: String = row.get(3)?;
        let protocol: String = row.get(4)?;
        endpoints.entry(row.get(0)?).or_default().push(Endpoint {
            slug: row.get(2)?,
            protocol: parse_protocol(&protocol)?,
            keys: keys.remove(&id).unwrap_or_default(),
            key_strategy: KeyStrategy::RoundRobin,
            probe_url: format!("{}/models", api_base(&base_url)),
            models: models.remove(&id).unwrap_or_default(),
            image_base_url: row.get(5)?,
            image_edit_base_url: row.get(7)?,
            headers: parse_headers(&row.get::<_, String>(6)?)?,
            base_url,
            id,
        });
    }
    let mut stmt = db.prepare_cached(
        "SELECT id, name, enabled, notes, preset_id FROM providers ORDER BY position, id",
    )?;
    let providers = stmt
        .query_map([], |row| {
            Ok(Provider {
                id: row.get(0)?,
                name: row.get(1)?,
                enabled: row.get(2)?,
                notes: row.get(3)?,
                preset_id: row.get(4)?,
                endpoints: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(providers
        .into_iter()
        .map(|provider| Provider {
            endpoints: endpoints.remove(&provider.id).unwrap_or_default(),
            ..provider
        })
        .collect())
}

fn parse_headers(json: &str) -> Result<BTreeMap<String, String>, KitError> {
    serde_json::from_str(json).map_err(|_| corrupt("headers"))
}

/// Ciphertexts of `endpoint_keys` rows matching `filter`, to `forget` after they are deleted.
fn ciphertexts(
    tx: &Db,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Vec<u8>>, KitError> {
    Ok(tx
        .prepare(&format!(
            "SELECT key_ciphertext FROM endpoint_keys WHERE {filter}"
        ))?
        .query_map(params, |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?)
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

    /// Keys of an endpoint in this request's try order.
    fn keys_of(store: &Store, endpoint_id: &str) -> Vec<String> {
        let connection = store.endpoint_connection(endpoint_id).unwrap();
        connection
            .api_keys
            .iter()
            .map(|key| key.as_str().to_owned())
            .collect()
    }

    fn save_key(store: &Store, value: Value) -> Result<Vec<Provider>, KitError> {
        store.save_key(serde_json::from_value(value).unwrap())
    }

    /// A saved provider with one endpoint `a` holding model `m1`; returns (provider id, endpoint id).
    fn saved(store: &Store) -> (String, String) {
        let saved = store
            .save(request(
                json!({ "name": "P", "endpoints": [endpoint(None, "a", &["m1"])] }),
            ))
            .unwrap();
        (saved[0].id.clone(), saved[0].endpoints[0].id.clone())
    }

    fn reason(error: &KitError) -> (KitErrorCode, Option<&str>, Option<&str>) {
        (
            error.code(),
            error.details().field(),
            error.details().reason(),
        )
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
        let expected = infer_reasoning(
            &reference.model_id,
            Protocol::AnthropicMessages,
            "https://api.anthropic.com/v1",
            None,
        )
        .unwrap();
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
    fn version_two_upgrade_preserves_models_and_native_declarations() {
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
                assert_eq!(
                    db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                        .unwrap(),
                    10
                );
            }
            assert!(path.with_file_name("providers.db.bak-v2").is_file());
        }
    }

    #[test]
    fn version_four_db_opens_with_old_keys_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("providers.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, key_ciphertext BLOB, position INTEGER NOT NULL, UNIQUE(provider_id, slug));
             CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, native_web_search INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint_id, model_id));
             INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1, 1);
             INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-chat', X'01020304', 0);
             INSERT INTO models VALUES ('e', 'm', 'chat', 'user', NULL, NULL, 0, 1, 0, NULL, 0);
             PRAGMA user_version = 4;",
        )
        .unwrap();
        drop(db);
        let store = Store::open(dir.path()).unwrap();
        let endpoint = &store.list().unwrap()[0].endpoints[0];
        assert_eq!((endpoint.id.as_str(), endpoint.models.len()), ("e", 1));
        assert!(endpoint.keys.is_empty());
        assert!(keys_of(&store, "e").is_empty());
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
            .save(request(
                json!({ "name": "Acme", "endpoints": [endpoint(None, "main", &["m1"])] }),
            ))
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
    fn keys_are_added_patched_revealed_and_removed() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        let list = save_key(
            &store,
            json!({ "endpointId": ep, "key": format!(" {KEY} ") }),
        )
        .unwrap();
        let key = list[0].endpoints[0].keys[0].clone();
        assert_eq!(
            (
                key.label.as_str(),
                key.enabled,
                key.readable,
                key.masked.as_deref()
            ),
            ("", true, true, Some("sk-t**************abcd"))
        );
        let reference = ModelRef {
            endpoint_id: ep.clone(),
            model_id: "m1".to_owned(),
        };
        let resolved = store.resolve(&reference).unwrap();
        assert_eq!(resolved.api_keys.len(), 1);
        assert_eq!(resolved.api_keys[0].as_str(), KEY);
        assert_eq!(store.reveal_key(&key.id).unwrap().as_str(), KEY);

        // Label-only patch keeps the ciphertext byte for byte.
        let cipher = |store: &Store| -> Vec<u8> {
            store
                .lock()
                .unwrap()
                .query_row("SELECT key_ciphertext FROM endpoint_keys", [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        let before = cipher(&store);
        let list = save_key(
            &store,
            json!({ "endpointId": ep, "id": key.id, "label": " main " }),
        )
        .unwrap();
        assert_eq!(list[0].endpoints[0].keys[0].label, "main");
        assert_eq!(cipher(&store), before);
        save_key(
            &store,
            json!({ "endpointId": ep, "id": key.id, "key": "sk-other-key-000000" }),
        )
        .unwrap();
        assert_eq!(keys_of(&store, &ep), ["sk-other-key-000000"]);

        assert!(
            store.remove_key(&ep, &key.id).unwrap()[0].endpoints[0]
                .keys
                .is_empty()
        );
        assert_eq!(
            reason(&store.remove_key(&ep, &key.id).unwrap_err()).0,
            KitErrorCode::NotFound
        );
        assert_eq!(
            reason(&store.reveal_key(&key.id).unwrap_err()).0,
            KitErrorCode::NotFound
        );
    }

    #[test]
    fn rotation_skips_disabled_keys_and_all_disabled_is_no_key() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        for key in ["k1", "k2", "k3"] {
            save_key(&store, json!({ "endpointId": ep, "key": key })).unwrap();
        }
        let ids: Vec<String> = store.list().unwrap()[0].endpoints[0]
            .keys
            .iter()
            .map(|k| k.id.clone())
            .collect();
        save_key(
            &store,
            json!({ "endpointId": ep, "id": ids[1], "enabled": false }),
        )
        .unwrap();
        let reference = ModelRef {
            endpoint_id: ep.clone(),
            model_id: "m1".to_owned(),
        };
        let firsts: Vec<String> = (0..4)
            .map(|_| {
                store.resolve(&reference).unwrap().api_keys[0]
                    .as_str()
                    .to_owned()
            })
            .collect();
        assert_eq!(firsts, ["k1", "k3", "k1", "k3"]);
        // Each request still carries every enabled key for failover, starting at the cursor.
        assert_eq!(keys_of(&store, &ep), ["k1", "k3"]);
        assert_eq!(keys_of(&store, &ep), ["k3", "k1"]);

        for id in [&ids[0], &ids[2]] {
            save_key(
                &store,
                json!({ "endpointId": ep, "id": id, "enabled": false }),
            )
            .unwrap();
        }
        assert!(store.resolve(&reference).unwrap().api_keys.is_empty());
    }

    #[test]
    fn explicit_snapshots_reserve_once_and_survive_configuration_deletion() {
        let (_dir, store) = store();
        let (provider, endpoint_id) = saved(&store);
        for key in ["first", "second"] {
            save_key(&store, json!({ "endpointId": endpoint_id, "key": key })).unwrap();
        }
        let reference = ModelRef {
            endpoint_id: endpoint_id.clone(),
            model_id: "m1".into(),
        };
        let snapshot = store.snapshot(&reference).unwrap();
        assert_eq!(snapshot.endpoint.api_keys[0].as_str(), "first");
        assert_eq!(snapshot.model.as_ref().unwrap().id, "m1");
        let next = store.endpoint_snapshot(&endpoint_id).unwrap();
        assert_eq!(next.api_keys[0].as_str(), "second");
        let prepared = connection::prepare(snapshot).unwrap();
        assert_eq!(prepared.api_keys[0].as_str(), "first");
        assert_eq!(
            store.endpoint_snapshot(&endpoint_id).unwrap().api_keys[0].as_str(),
            "first"
        );
        store.delete(&provider).unwrap();
        let prepared = connection::prepare(ConnectionSnapshot {
            endpoint: next,
            model: None,
        })
        .unwrap();
        assert_eq!(prepared.api_keys[0].as_str(), "second");
        assert_eq!(
            store.snapshot(&reference).err().unwrap().details().entity(),
            Some("model")
        );
    }

    #[test]
    fn invalid_keys_are_rejected() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        let id = store.list().unwrap()[0].endpoints[0].keys[0].id.clone();
        let cases = [
            (json!({ "endpointId": ep }), ("apiKey", "empty")),
            (
                json!({ "endpointId": ep, "key": "  " }),
                ("apiKey", "empty"),
            ),
            (
                json!({ "endpointId": ep, "id": id, "key": "" }),
                ("apiKey", "empty"),
            ),
            (
                json!({ "endpointId": ep, "key": "k".repeat(4097) }),
                ("apiKey", "too_long"),
            ),
            (
                json!({ "endpointId": ep, "key": format!("{KEY} ") }),
                ("apiKey", "duplicate"),
            ),
            (
                json!({ "endpointId": ep, "key": "k2", "label": "x".repeat(41) }),
                ("label", "too_long"),
            ),
        ];
        for (input, (field, why)) in cases {
            let error = save_key(&store, input).unwrap_err();
            assert_eq!(
                reason(&error),
                (KitErrorCode::BadRequest, Some(field), Some(why))
            );
            assert_eq!(store.reveal_key(&id).unwrap().as_str(), KEY);
            assert_eq!(keys_of(&store, &ep), [KEY]);
        }
        // Re-saving a key's own value is not a duplicate of itself.
        save_key(&store, json!({ "endpointId": ep, "id": id, "key": KEY })).unwrap();
        let missing = save_key(&store, json!({ "endpointId": "nope", "key": "k2" })).unwrap_err();
        assert_eq!(missing.details().entity(), Some("endpoint"));
        let missing = save_key(
            &store,
            json!({ "endpointId": ep, "id": "nope", "label": "x" }),
        )
        .unwrap_err();
        assert_eq!(missing.details().entity(), Some("key"));
        for n in 1..16 {
            save_key(&store, json!({ "endpointId": ep, "key": format!("k-{n}") })).unwrap();
        }
        let error = save_key(&store, json!({ "endpointId": ep, "key": "k-17" })).unwrap_err();
        assert_eq!(
            reason(&error),
            (KitErrorCode::BadRequest, Some("keys"), Some("too_many"))
        );
        assert_eq!(store.list().unwrap()[0].endpoints[0].keys.len(), 16);
    }

    #[test]
    fn deleting_endpoints_and_providers_removes_their_keys() {
        let (_dir, store) = store();
        let count = |store: &Store| -> i64 {
            store
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM endpoint_keys", [], |row| row.get(0))
                .unwrap()
        };
        let (provider, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        store
            .save(request(
                json!({ "id": provider, "name": "P", "endpoints": [endpoint(None, "b", &[])] }),
            ))
            .unwrap();
        assert_eq!(count(&store), 0);
        let (provider, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        store.delete(&provider).unwrap();
        assert_eq!(count(&store), 0);
    }

    #[test]
    fn empty_address_saves_but_does_not_resolve() {
        let (_dir, store) = store();
        let mut ep = endpoint(None, "a", &["m1"]);
        ep["baseUrl"] = json!("");
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [ep] })))
            .unwrap();
        let reference = ModelRef {
            endpoint_id: saved[0].endpoints[0].id.clone(),
            model_id: "m1".to_owned(),
        };
        let error = store.resolve(&reference).unwrap_err();
        assert_eq!(
            reason(&error),
            (KitErrorCode::NeedsConfig, None, Some("missing_url"))
        );
    }

    #[test]
    fn unreadable_ciphertext_is_reported_not_fatal() {
        let (_dir, store) = store();
        let (_, ep) = saved(&store);
        save_key(&store, json!({ "endpointId": ep, "key": KEY })).unwrap();
        // Unknown format marker, as after copying the database to another platform.
        store
            .lock()
            .unwrap()
            .execute("UPDATE endpoint_keys SET key_ciphertext = X'7F0102'", [])
            .unwrap();
        let key = &store.list().unwrap()[0].endpoints[0].keys[0];
        assert!(!key.readable && key.masked.is_none());
        let error = store.endpoint_connection(&ep).err().unwrap();
        assert_eq!(
            reason(&error),
            (KitErrorCode::NeedsConfig, None, Some("key_unreadable"))
        );
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

    #[test]
    fn version_five_db_upgrades_with_empty_request_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("providers.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, position INTEGER NOT NULL, UNIQUE(provider_id, slug));
             CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, native_web_search INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint_id, model_id));
             CREATE TABLE endpoint_keys (id TEXT PRIMARY KEY, endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, label TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1, key_ciphertext BLOB NOT NULL, position INTEGER NOT NULL);
             INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1, 1);
             INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-chat', 0);
             PRAGMA user_version = 5;",
        )
        .unwrap();
        drop(db);
        let store = Store::open(dir.path()).unwrap();
        let endpoint = &store.list().unwrap()[0].endpoints[0];
        assert_eq!(endpoint.image_base_url, None);
        assert_eq!(endpoint.image_edit_base_url, None);
        assert!(endpoint.headers.is_empty());
        let connection = store.endpoint_connection("e").unwrap();
        assert!(connection.headers.is_empty() && connection.image_base_url.is_none());
        let version: u32 = store
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
    }

    #[test]
    fn version_six_db_upgrades_to_seven_keeping_request_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("providers.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE endpoints (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE, slug TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL, position INTEGER NOT NULL, image_base_url TEXT, headers TEXT NOT NULL DEFAULT '{}', UNIQUE(provider_id, slug));
             CREATE TABLE models (endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, model_id TEXT NOT NULL, model_type TEXT NOT NULL, type_source TEXT NOT NULL, context_window INTEGER, max_output INTEGER, can_vision INTEGER NOT NULL DEFAULT 0, supports_tools INTEGER, position INTEGER NOT NULL, reasoning TEXT, native_web_search INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(endpoint_id, model_id));
             CREATE TABLE endpoint_keys (id TEXT PRIMARY KEY, endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, label TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1, key_ciphertext BLOB NOT NULL, position INTEGER NOT NULL);
             CREATE INDEX idx_endpoint_keys ON endpoint_keys(endpoint_id, position, id);
             INSERT INTO providers VALUES ('p', 'Saved', 0, 1, 1, 1);
             INSERT INTO endpoints VALUES ('e', 'p', 'saved', 'https://example.com', 'openai-chat', 0, 'https://img.example.com', '{\"X-A\":\"1\"}');
             PRAGMA user_version = 6;",
        )
        .unwrap();
        drop(db);
        let store = Store::open(dir.path()).unwrap();
        let endpoint = &store.list().unwrap()[0].endpoints[0];
        assert_eq!(
            endpoint.image_base_url.as_deref(),
            Some("https://img.example.com")
        );
        assert_eq!(endpoint.image_edit_base_url, None);
        assert_eq!(endpoint.headers.len(), 1);
        assert!(
            store
                .endpoint_connection("e")
                .unwrap()
                .image_edit_base_url
                .is_none()
        );
        let version: u32 = store
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
        assert_eq!(store.list().unwrap()[0].notes, "");
    }

    #[test]
    fn version_nine_db_upgrades_to_ten_with_empty_display_and_pricing() {
        let (dir, store) = store();
        let (_, endpoint_id) = saved(&store);
        store
            .lock()
            .unwrap()
            .execute_batch(
                "ALTER TABLE models DROP COLUMN name; ALTER TABLE models DROP COLUMN group_name;
                 ALTER TABLE models DROP COLUMN pricing; PRAGMA user_version = 9;",
            )
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let version: u32 = store
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
        let model = &store.list().unwrap()[0].endpoints[0].models[0];
        assert_eq!(
            (
                model.id.as_str(),
                &model.name,
                &model.group_name,
                &model.pricing
            ),
            ("m1", &None, &None, &None)
        );
        let reference = ModelRef {
            endpoint_id,
            model_id: "m1".into(),
        };
        assert!(store.resolve(&reference).unwrap().pricing.is_none());
    }

    #[test]
    fn display_fields_and_pricing_round_trip_to_every_read_path() {
        let (dir, store) = store();
        let mut e = endpoint(None, "a", &["m1"]);
        let pricing = json!({ "currency": "CNY", "tiers": [
            { "minInputTokens": 0, "input": 2.0, "output": 8.0, "cacheRead": 0.5, "cacheWrite": null },
            { "minInputTokens": 32000, "input": 4.0, "output": 16.0, "cacheRead": null, "cacheWrite": null }
        ] });
        e["models"][0]["name"] = json!("  Model One  ");
        e["models"][0]["groupName"] = json!("   ");
        e["models"][0]["pricing"] = pricing.clone();
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [e] })))
            .unwrap();
        let reference = ModelRef {
            endpoint_id: saved[0].endpoints[0].id.clone(),
            model_id: "m1".into(),
        };
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let model = &store.list().unwrap()[0].endpoints[0].models[0];
        assert_eq!(model.name.as_deref(), Some("Model One"));
        assert_eq!(model.group_name, None);
        assert_eq!(serde_json::to_value(&model.pricing).unwrap(), pricing);
        let option = &store.model_options(ModelFilter::default()).unwrap()[0];
        assert_eq!(option.name.as_deref(), Some("Model One"));
        assert_eq!(option.label, "P · a · m1");
        assert_eq!(
            serde_json::to_value(store.resolve(&reference).unwrap().pricing).unwrap(),
            pricing
        );
    }

    #[test]
    fn null_reasoning_turns_chat_reasoning_off_without_reinference() {
        let (dir, store) = store();
        let mut e = endpoint(None, "a", &["claude-sonnet-5-5", "embed"]);
        e["protocol"] = json!("anthropic-messages");
        e["models"][0]["reasoning"] = Value::Null;
        e["models"][1]["modelType"] = json!("embedding");
        e["models"][1]["reasoning"] = Value::Null;
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [e.clone()] })))
            .unwrap();
        let id = saved[0].endpoints[0].id.clone();
        let stored = |store: &Store| -> Vec<Option<String>> {
            store
                .lock()
                .unwrap()
                .prepare("SELECT reasoning FROM models ORDER BY position")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(stored(&store), vec![Some("none".to_owned()), None]);
        // Omitted keeps the stored 'none'.
        e["id"] = json!(id);
        for model in e["models"].as_array_mut().unwrap() {
            model.as_object_mut().unwrap().remove("reasoning");
        }
        store
            .save(request(
                json!({ "id": saved[0].id, "name": "P", "endpoints": [e] }),
            ))
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(stored(&store), vec![Some("none".to_owned()), None]);
        let reference = ModelRef {
            endpoint_id: id,
            model_id: "claude-sonnet-5-5".into(),
        };
        assert!(
            store.list().unwrap()[0].endpoints[0].models[0]
                .reasoning
                .is_none()
        );
        assert!(store.resolve(&reference).unwrap().reasoning.is_none());
        assert!(
            store.model_options(ModelFilter::default()).unwrap()[0]
                .reasoning
                .is_none()
        );
    }

    #[test]
    fn kept_reasoning_is_revalidated_against_the_new_protocol_and_output() {
        let (_dir, store) = store();
        let mut e = endpoint(None, "a", &["m1"]);
        e["protocol"] = json!("anthropic-messages");
        e["models"][0]["maxOutput"] = json!(8000);
        e["models"][0]["reasoning"] =
            json!({ "dialect": "anthropic-budget", "efforts": ["high"], "budgetTokens": 4000 });
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [e.clone()] })))
            .unwrap();
        e["id"] = json!(saved[0].endpoints[0].id);
        e["models"][0].as_object_mut().unwrap().remove("reasoning");
        let resave = |e: &Value| {
            store
                .save(request(
                    json!({ "id": saved[0].id, "name": "P", "endpoints": [e] }),
                ))
                .err()
                .map(|error| reason(&error).2.map(str::to_owned))
        };
        let mut lower = e.clone();
        lower["models"][0]["maxOutput"] = json!(4000);
        assert_eq!(resave(&lower), Some(Some("invalid_budget".to_owned())));
        let mut openai = e.clone();
        openai["protocol"] = json!("openai-chat");
        assert_eq!(resave(&openai), Some(Some("protocol_mismatch".to_owned())));
        let mut image = e.clone();
        image["models"][0]["modelType"] = json!("image");
        assert_eq!(resave(&image), Some(Some("invalid_efforts".to_owned())));
        assert_eq!(resave(&e), None);
    }

    #[test]
    fn invalid_model_settings_are_rejected_with_contract_reasons() {
        let (_dir, store) = store();
        let tier = |min: u64, price: f64| json!({ "minInputTokens": min, "input": price, "output": 1, "cacheRead": null, "cacheWrite": null });
        let pricing = |tiers: Vec<Value>| json!({ "currency": "USD", "tiers": tiers });
        let long = "x".repeat(81);
        let cases = [
            (json!({ "name": long }), "openai-chat", "name", "too_long"),
            (
                json!({ "groupName": long }),
                "openai-chat",
                "groupName",
                "too_long",
            ),
            (
                json!({ "contextWindow": 0 }),
                "openai-chat",
                "contextWindow",
                "invalid",
            ),
            (
                json!({ "maxOutput": 0 }),
                "openai-chat",
                "maxOutput",
                "invalid",
            ),
            (
                json!({ "maxOutput": 200_000 }),
                "openai-chat",
                "maxOutput",
                "exceeds_context",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": [] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": ["low", "low"] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": ["default"] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "modelType": "image", "reasoning": { "dialect": "openai", "efforts": ["low"] } }),
                "openai-chat",
                "reasoning",
                "invalid_efforts",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-adaptive", "efforts": ["low"] } }),
                "openai-chat",
                "reasoning",
                "protocol_mismatch",
            ),
            (
                json!({ "reasoning": { "dialect": "openai", "efforts": ["low"] } }),
                "anthropic-messages",
                "reasoning",
                "protocol_mismatch",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-budget", "efforts": ["low"] } }),
                "anthropic-messages",
                "reasoning",
                "invalid_budget",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-budget", "efforts": ["low"], "budgetTokens": 1023 } }),
                "anthropic-messages",
                "reasoning",
                "invalid_budget",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-budget", "efforts": ["low"], "budgetTokens": 4096 } }),
                "anthropic-messages",
                "reasoning",
                "invalid_budget",
            ),
            (
                json!({ "reasoning": { "dialect": "anthropic-adaptive", "efforts": ["low"], "budgetTokens": 2000 } }),
                "anthropic-messages",
                "reasoning",
                "unexpected_budget",
            ),
            (
                json!({ "modelType": "image", "pricing": pricing(vec![tier(0, 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing((0..9).map(|i| tier(i * 10, 1.0)).collect()) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(5, 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(0, 1.0), tier(0, 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(0, -1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
            (
                json!({ "pricing": pricing(vec![tier(0, 1e6 + 1.0)]) }),
                "openai-chat",
                "pricing",
                "invalid",
            ),
        ];
        for (extra, protocol, field, why) in cases {
            let mut value = endpoint(None, "a", &["m1"]);
            value["protocol"] = json!(protocol);
            for (k, v) in extra.as_object().unwrap() {
                value["models"][0][k] = v.clone();
            }
            let error = store
                .save(request(json!({ "name": "P", "endpoints": [value] })))
                .err()
                .unwrap();
            assert_eq!(
                reason(&error),
                (KitErrorCode::BadRequest, Some(field), Some(why)),
                "{extra}"
            );
        }
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn presets_seed_once_adopt_same_name_and_cannot_be_deleted() {
        let (_dir, store) = store();
        let seed = |name: &str, preset: &str| {
            store.save(request(
                json!({ "name": name, "presetId": preset, "enabled": false,
                "endpoints": [endpoint(None, "ep-1", &[])] }),
            ))
        };
        let seeded = seed("七牛云", "qiniu").unwrap();
        assert_eq!(seeded.len(), 1);
        assert_eq!(seeded[0].preset_id.as_deref(), Some("qiniu"));
        assert!(!seeded[0].enabled);
        assert_eq!(seed("七牛云", "qiniu").unwrap().len(), 1);
        let id = seeded[0].id.clone();
        let renamed = store
            .save(request(
                json!({ "id": id, "name": "Q", "presetId": "other", "endpoints": [] }),
            ))
            .unwrap();
        assert_eq!(renamed[0].preset_id.as_deref(), Some("qiniu"));
        let error = store.delete(&id).unwrap_err();
        assert_eq!(
            reason(&error),
            (KitErrorCode::Conflict, None, Some("preset_provider"))
        );
        assert_eq!(store.list().unwrap().len(), 1);

        let own = store
            .save(request(
                json!({ "name": "Fenno", "endpoints": [endpoint(None, "mine", &[])] }),
            ))
            .unwrap();
        let fenno = own.iter().find(|p| p.name == "Fenno").unwrap().id.clone();
        let adopted = seed("fenno", "fenno").unwrap();
        assert_eq!(adopted.len(), 2);
        let fenno = adopted.iter().find(|p| p.id == fenno).unwrap();
        assert_eq!(fenno.preset_id.as_deref(), Some("fenno"));
        assert_eq!(fenno.endpoints[0].slug, "mine");
    }

    #[test]
    fn notes_save_keep_when_omitted_and_reject_oversized() {
        let (_dir, store) = store();
        let saved = store
            .save(request(
                json!({ "name": "P", "notes": "## 价格\n- 便宜", "endpoints": [] }),
            ))
            .unwrap();
        let id = saved[0].id.clone();
        assert_eq!(saved[0].notes, "## 价格\n- 便宜");
        let kept = store
            .save(request(json!({ "id": id, "name": "P2", "endpoints": [] })))
            .unwrap();
        assert_eq!(kept[0].notes, "## 价格\n- 便宜");
        let cleared = store
            .save(request(
                json!({ "id": id, "name": "P2", "notes": "", "endpoints": [] }),
            ))
            .unwrap();
        assert_eq!(cleared[0].notes, "");
        let error = store
            .save(request(
                json!({ "id": id, "name": "P2", "notes": "x".repeat(10_001), "endpoints": [] }),
            ))
            .unwrap_err();
        assert_eq!(reason(&error).1, Some("notes"));
    }

    #[test]
    fn request_config_round_trips_and_omitted_fields_keep() {
        let (_dir, store) = store();
        let mut first = endpoint(None, "a", &["m1"]);
        first["imageBaseUrl"] = json!("  https://img.example.com/  ");
        first["imageEditBaseUrl"] = json!(" https://edit.example.com ");
        first["headers"] = json!({ " X-Relay-Tag ": "team-a", "X-Two": "2" });
        let saved = store
            .save(request(json!({ "name": "P", "endpoints": [first] })))
            .unwrap();
        let (provider, ep) = (saved[0].id.clone(), saved[0].endpoints[0].id.clone());
        let expected = BTreeMap::from([
            ("X-Relay-Tag".to_owned(), "team-a".to_owned()),
            ("X-Two".to_owned(), "2".to_owned()),
        ]);
        let check = |store: &Store| {
            let listed = &store.list().unwrap()[0].endpoints[0];
            assert_eq!(
                listed.image_base_url.as_deref(),
                Some("https://img.example.com/")
            );
            assert_eq!(
                listed.image_edit_base_url.as_deref(),
                Some("https://edit.example.com")
            );
            assert_eq!(listed.headers, expected);
            let resolved = store
                .resolve(&ModelRef {
                    endpoint_id: ep.clone(),
                    model_id: "m1".to_owned(),
                })
                .unwrap();
            assert_eq!(
                resolved.image_base_url.as_deref(),
                Some("https://img.example.com/v1")
            );
            assert_eq!(
                resolved.image_edit_base_url.as_deref(),
                Some("https://edit.example.com/v1")
            );
            assert_eq!(resolved.headers.len(), 2);
        };
        check(&store);
        // Omitted = keep.
        let kept = endpoint(Some(&ep), "a", &["m1"]);
        store
            .save(request(
                json!({ "id": provider, "name": "P", "endpoints": [kept] }),
            ))
            .unwrap();
        check(&store);
        // null / "" and {} clear.
        for image in [json!(null), json!("  ")] {
            let mut cleared = endpoint(Some(&ep), "a", &["m1"]);
            cleared["imageBaseUrl"] = image.clone();
            cleared["imageEditBaseUrl"] = image;
            cleared["headers"] = json!({});
            let listed = store
                .save(request(
                    json!({ "id": provider, "name": "P", "endpoints": [cleared] }),
                ))
                .unwrap();
            assert_eq!(listed[0].endpoints[0].image_base_url, None);
            assert_eq!(listed[0].endpoints[0].image_edit_base_url, None);
            assert!(listed[0].endpoints[0].headers.is_empty());
        }
    }

    #[test]
    fn invalid_request_config_is_rejected() {
        let (_dir, store) = store();
        let many: serde_json::Map<String, Value> =
            (0..21).map(|i| (format!("x-{i}"), json!("v"))).collect();
        let cases = [
            (
                json!({ "imageBaseUrl": "ftp://x" }),
                "imageBaseUrl",
                "invalid_url",
            ),
            (
                json!({ "imageBaseUrl": format!("https://{}", "a".repeat(2048)) }),
                "imageBaseUrl",
                "invalid_url",
            ),
            (
                json!({ "imageEditBaseUrl": "ftp://x" }),
                "imageEditBaseUrl",
                "invalid_url",
            ),
            (
                json!({ "imageEditBaseUrl": format!("https://{}", "a".repeat(2048)) }),
                "imageEditBaseUrl",
                "invalid_url",
            ),
            (json!({ "headers": many }), "headers", "too_many"),
            (
                json!({ "headers": { "bad name": "v" } }),
                "headers",
                "invalid",
            ),
            (json!({ "headers": { " ": "v" } }), "headers", "invalid"),
            (
                json!({ "headers": { "X-A": "a\r\nb" } }),
                "headers",
                "invalid",
            ),
            (
                json!({ "headers": { "X-A": "v".repeat(2049) } }),
                "headers",
                "invalid",
            ),
            (
                json!({ "headers": { "X-A": "1", "x-a": "2" } }),
                "headers",
                "invalid",
            ),
            (
                json!({ "headers": { "Authorization": "Bearer x" } }),
                "headers",
                "reserved",
            ),
            (
                json!({ "headers": { "X-API-Key": "x" } }),
                "headers",
                "reserved",
            ),
            (
                json!({ "headers": { "cookie": "x" } }),
                "headers",
                "reserved",
            ),
        ];
        for (extra, field, why) in cases {
            let mut value = endpoint(None, "a", &[]);
            for (k, v) in extra.as_object().unwrap() {
                value[k] = v.clone();
            }
            let error = store
                .save(request(json!({ "name": "P", "endpoints": [value] })))
                .err()
                .unwrap();
            assert_eq!(
                reason(&error),
                (KitErrorCode::BadRequest, Some(field), Some(why)),
                "{extra}"
            );
        }
        assert!(store.list().unwrap().is_empty());
    }
}
