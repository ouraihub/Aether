//! SQLite configuration, encrypted credentials and per-capability key snapshots.
use crate::{
    AddressPolicy, CheckResult, SearchCap, SearchKey, SearchPage, SearchPreferences,
    SearchProvider, SearchResult, SearchSave, SearchSettings,
    execution::ResolvedConfig,
    policy::validate_preferences,
    vendor::{PROVIDERS, default_provider, lookup},
};
use aether_support::{
    KitError, KitErrorCode,
    db::{self, Migration},
    secret,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::HashMap, path::Path, sync::Mutex};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "CREATE TABLE search_settings(capability TEXT PRIMARY KEY,provider TEXT NOT NULL); CREATE TABLE search_providers(capability TEXT NOT NULL,provider TEXT NOT NULL,base_url TEXT,basic_username TEXT,PRIMARY KEY(capability,provider)); CREATE TABLE search_keys(id TEXT PRIMARY KEY,capability TEXT NOT NULL,provider TEXT NOT NULL,key_ciphertext BLOB NOT NULL,position INTEGER NOT NULL); CREATE TABLE search_preferences(id INTEGER PRIMARY KEY CHECK(id=1),json TEXT NOT NULL);",
}];
type Row = (Option<String>, Option<String>);
/// SQLite configuration store; locks are released before network operations.
pub struct Store {
    db: Mutex<Connection>,
    policy: AddressPolicy,
    cursors: Mutex<HashMap<String, usize>>,
}
impl Store {
    /// Opens a new-schema search database without migrating historical credentials.
    /// # Errors
    /// Rejects incompatible databases and storage failures.
    pub fn open(path: &Path, policy: AddressPolicy) -> Result<Self, KitError> {
        let conn = db::open(path, MIGRATIONS)?;
        if conn
            .prepare("SELECT key_ciphertext FROM search_keys LIMIT 0")
            .is_err()
            || conn
                .prepare("SELECT json FROM search_preferences LIMIT 0")
                .is_err()
        {
            return Err(KitError::new(
                KitErrorCode::IncompatibleDb,
                "search database uses an incompatible schema",
            ));
        }
        Ok(Self {
            db: Mutex::new(conn),
            policy,
            cursors: Mutex::new(HashMap::new()),
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("search database lock poisoned")))
    }
    fn row(conn: &Connection, cap: SearchCap, id: &str) -> Result<Option<Row>, KitError> {
        Ok(conn.query_row("SELECT base_url,basic_username FROM search_providers WHERE capability=?1 AND provider=?2", params![cap.key(), id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }
    fn keys(
        conn: &Connection,
        cap: SearchCap,
        provider: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, KitError> {
        let mut stmt = conn.prepare("SELECT id,key_ciphertext FROM search_keys WHERE capability=?1 AND provider=?2 ORDER BY position,id")?;
        Ok(stmt
            .query_map(params![cap.key(), provider], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?)
    }
    fn read_preferences(conn: &Connection) -> Result<SearchPreferences, KitError> {
        let json: Option<String> = conn
            .query_row("SELECT json FROM search_preferences WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?;
        let preferences = match json {
            Some(s) => serde_json::from_str(&s).map_err(KitError::internal)?,
            None => SearchPreferences::default(),
        };
        validate_preferences(&preferences)?;
        Ok(preferences)
    }
    /// Reads global search policy for external and model-native routing.
    /// # Errors
    /// Reports invalid persisted preferences or storage failures.
    pub fn preferences(&self) -> Result<SearchPreferences, KitError> {
        let db = self.lock()?;
        Self::read_preferences(&db)
    }
    /// Persists the entire validated global policy atomically.
    /// # Errors
    /// Rejects invalid bounds or blacklist syntax without changing existing policy.
    pub fn save_preferences(
        &self,
        input: SearchPreferences,
    ) -> Result<SearchPreferences, KitError> {
        validate_preferences(&input)?;
        let json = serde_json::to_string(&input).map_err(KitError::internal)?;
        self.lock()?.execute("INSERT INTO search_preferences(id,json)VALUES(1,?1) ON CONFLICT(id)DO UPDATE SET json=excluded.json", [json])?;
        Ok(input)
    }
    /// Lists provider metadata and safe masked key records.
    /// # Errors
    /// Reports storage failures; unreadable keys stay visible as unavailable.
    pub fn settings(&self, cap: SearchCap) -> Result<SearchSettings, KitError> {
        let db = self.lock()?;
        let default = db
            .query_row(
                "SELECT provider FROM search_settings WHERE capability=?1",
                [cap.key()],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| default_provider(cap).into());
        let mut providers = Vec::new();
        for spec in PROVIDERS {
            let Some((base0, mode)) = (match cap {
                SearchCap::Search => spec.search,
                SearchCap::Fetch => spec.fetch,
            }) else {
                continue;
            };
            let (base, user) = Self::row(&db, cap, spec.id)?.unwrap_or((None, None));
            let mut readable = false;
            let keys = Self::keys(&db, cap, spec.id)?
                .into_iter()
                .map(|(id, cipher)| {
                    let masked = match secret::open(&cipher) {
                        Ok(key) => {
                            readable = true;
                            aether_support::mask::mask_secret(&key)
                        }
                        Err(_) => "[unreadable]".into(),
                    };
                    SearchKey { id, masked }
                })
                .collect();
            providers.push(SearchProvider {
                id: spec.id.into(),
                name: spec.name.into(),
                description: spec.description.into(),
                official_website: (!spec.website.is_empty()).then(|| spec.website.into()),
                api_key_website: spec.key_site.map(ToOwned::to_owned),
                key_mode: mode.into(),
                basic_auth: spec.basic,
                default_base_url: (!base0.is_empty()).then(|| base0.into()),
                base_url: (!base0.is_empty()).then(|| {
                    base.filter(|s| !s.is_empty())
                        .unwrap_or_else(|| base0.into())
                }),
                keys,
                basic_auth_username: user,
                ready: mode != "required" || readable,
            });
        }
        Ok(SearchSettings {
            capability: cap,
            default_provider: default,
            preferences: Self::read_preferences(&db)?,
            providers,
        })
    }
    /// Updates only provider address and username; credentials are separate records.
    /// # Errors
    /// Rejects unsafe addresses and oversized input.
    pub fn save(&self, input: SearchSave) -> Result<SearchSettings, KitError> {
        let (spec, base0, _) = lookup(input.capability, &input.provider)?;
        if input.base_url.as_ref().is_some_and(|s| s.len() > 8192)
            || input
                .basic_auth_username
                .as_ref()
                .is_some_and(|s| s.len() > 200)
        {
            return Err(KitError::bad_request("provider", "too_long"));
        }
        if let Some(base) = input.base_url.as_deref().filter(|s| !s.trim().is_empty())
            && !base0.is_empty()
        {
            aether_support::net::check_url(base.trim(), &self.policy)?;
        }
        let db = self.lock()?;
        let old = Self::row(&db, input.capability, spec.id)?.unwrap_or((None, None));
        let base = match input.base_url {
            _ if base0.is_empty() => None,
            None => old.0,
            Some(s) => {
                let s = s.trim().trim_end_matches('/');
                (!s.is_empty()).then(|| s.to_owned())
            }
        };
        let user = if spec.basic {
            input
                .basic_auth_username
                .map(|s| s.trim().to_owned())
                .or(old.1)
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        db.execute("INSERT INTO search_providers(capability,provider,base_url,basic_username)VALUES(?1,?2,?3,?4) ON CONFLICT(capability,provider)DO UPDATE SET base_url=excluded.base_url,basic_username=excluded.basic_username", params![input.capability.key(), spec.id, base, user])?;
        drop(db);
        self.settings(input.capability)
    }
    /// Changes the default provider without changing global preferences.
    /// # Errors
    /// Reports unsupported providers or storage failures.
    pub fn set_default(&self, cap: SearchCap, provider: &str) -> Result<SearchSettings, KitError> {
        lookup(cap, provider)?;
        self.lock()?.execute("INSERT INTO search_settings(capability,provider)VALUES(?1,?2) ON CONFLICT(capability)DO UPDATE SET provider=excluded.provider", params![cap.key(), provider])?;
        self.settings(cap)
    }
    /// Adds or replaces one encrypted credential in its capability/provider scope.
    /// # Errors
    /// Rejects empty/oversized keys, missing scoped IDs, or more than sixteen keys.
    pub fn save_key(
        &self,
        cap: SearchCap,
        provider: &str,
        value: &str,
        key_id: Option<&str>,
    ) -> Result<SearchSettings, KitError> {
        let (spec, _, mode) = lookup(cap, provider)?;
        let value = value.trim();
        if value.is_empty()
            || value.encode_utf16().count() > 4096
            || (mode == "none" && !spec.basic)
        {
            return Err(KitError::bad_request("value", "invalid"));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if let Some(id) = key_id {
            let exists = tx
                .query_row(
                    "SELECT 1 FROM search_keys WHERE id=?1 AND capability=?2 AND provider=?3",
                    params![id, cap.key(), provider],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                return Err(KitError::not_found("key"));
            }
            tx.execute("UPDATE search_keys SET key_ciphertext=?1 WHERE id=?2 AND capability=?3 AND provider=?4", params![secret::seal(value)?, id, cap.key(), provider])?;
        } else {
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM search_keys WHERE capability=?1 AND provider=?2",
                params![cap.key(), provider],
                |r| r.get(0),
            )?;
            if count >= 16 {
                return Err(KitError::bad_request("keys", "too_many"));
            }
            tx.execute("INSERT INTO search_keys(id,capability,provider,key_ciphertext,position)SELECT lower(hex(randomblob(16))),?1,?2,?3,coalesce(max(position)+1,0) FROM search_keys WHERE capability=?1 AND provider=?2", params![cap.key(), provider, secret::seal(value)?])?;
        }
        tx.commit()?;
        drop(db);
        self.settings(cap)
    }
    /// Explicitly removes one scoped key.
    /// # Errors
    /// Rejects unsupported providers or missing scoped keys.
    pub fn remove_key(
        &self,
        cap: SearchCap,
        provider: &str,
        key_id: &str,
    ) -> Result<SearchSettings, KitError> {
        lookup(cap, provider)?;
        if self.lock()?.execute(
            "DELETE FROM search_keys WHERE id=?1 AND capability=?2 AND provider=?3",
            params![key_id, cap.key(), provider],
        )? == 0
        {
            return Err(KitError::not_found("key"));
        }
        self.settings(cap)
    }
    /// Returns one clear credential only on explicit scoped request.
    /// # Errors
    /// Reports missing keys or decryption failure.
    pub fn reveal(&self, cap: SearchCap, provider: &str, key_id: &str) -> Result<String, KitError> {
        lookup(cap, provider)?;
        let cipher: Vec<u8> = self.lock()?.query_row("SELECT key_ciphertext FROM search_keys WHERE id=?1 AND capability=?2 AND provider=?3", params![key_id, cap.key(), provider], |r| r.get(0)).optional()?.ok_or_else(|| KitError::not_found("key"))?;
        Ok(secret::open(&cipher)?.to_string())
    }
    /// Resolves and reserves this request's credential order without sending HTTP.
    /// # Errors
    /// Reports unsupported providers, unreadable/missing credentials or storage failures.
    pub fn resolve(
        &self,
        cap: SearchCap,
        provider: Option<&str>,
    ) -> Result<ResolvedConfig, KitError> {
        let db = self.lock()?;
        let selected = db
            .query_row(
                "SELECT provider FROM search_settings WHERE capability=?1",
                [cap.key()],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| default_provider(cap).into());
        let provider = provider.unwrap_or(&selected);
        let (_, base0, mode) = lookup(cap, provider)?;
        let (base, user) = Self::row(&db, cap, provider)?.unwrap_or((None, None));
        let rows = Self::keys(&db, cap, provider)?;
        let mut keys = Vec::new();
        for (_, cipher) in &rows {
            if let Ok(key) = secret::open(cipher) {
                keys.push(key);
            }
        }
        if !rows.is_empty() && keys.is_empty() {
            return Err(KitError::needs_config("keys"));
        }
        if mode == "required" && keys.is_empty() {
            return Err(KitError::needs_config("keys"));
        }
        let prefs = Self::read_preferences(&db)?;
        if !keys.is_empty() {
            let mut cursors = self.cursors.lock().map_err(|_| {
                KitError::internal(std::io::Error::other("search cursor lock poisoned"))
            })?;
            let cursor = cursors
                .entry(format!("{}:{provider}", cap.key()))
                .or_default();
            let start = *cursor % keys.len();
            *cursor = (start + 1) % keys.len();
            keys.rotate_left(start);
        }
        Ok(ResolvedConfig {
            provider: provider.into(),
            base_url: base
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| base0.into()),
            api_keys: keys,
            basic_auth_username: user,
            preferences: prefs,
            address_policy: self.policy.clone(),
        })
    }
    /// Searches using round-robin credentials and global result processing.
    /// # Errors
    /// Reports invalid input, configuration, network and upstream failures.
    pub async fn search(&self, query: &str) -> Result<SearchResult, KitError> {
        let query = crate::search::validate_query(query)?;
        crate::search::search(self.resolve(SearchCap::Search, None)?, query).await
    }
    /// Reads through the selected provider with the same bounded credential retries.
    /// # Errors
    /// Reports unsafe URLs, missing configuration and upstream failures.
    pub async fn fetch(&self, url: &str) -> Result<SearchPage, KitError> {
        let url = crate::fetch::validate_url(url, &self.policy)?;
        crate::fetch::fetch(self.resolve(SearchCap::Fetch, None)?, url).await
    }
    /// Checks saved settings through the same executor used by normal requests.
    /// # Errors
    /// Reports configuration, upstream failures or no-result searches.
    pub async fn check(&self, cap: SearchCap, provider: &str) -> Result<CheckResult, KitError> {
        let config = self.resolve(cap, Some(provider))?;
        let count = match cap {
            SearchCap::Search => crate::search::search(config, "web search")
                .await?
                .hits
                .len(),
            SearchCap::Fetch => {
                crate::fetch::fetch(config, "https://example.com").await?;
                1
            }
        };
        if count == 0 {
            return Err(KitError::new(
                KitErrorCode::Upstream,
                "provider returned no results",
            ));
        }
        Ok(CheckResult { count })
    }
}
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
