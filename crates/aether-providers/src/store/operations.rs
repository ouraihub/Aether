use super::*;

impl Store {
    // Store write operations.
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
}
