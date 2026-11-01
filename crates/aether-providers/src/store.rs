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
    fn lock(&self) -> Result<MutexGuard<'_, Db>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("providers store lock poisoned")))
    }
}

mod helpers;
mod operations;
mod reads;
use helpers::*;

// Keep the migration/opening implementation in this file; operations and reads extend Store.

#[cfg(test)]
mod tests;
