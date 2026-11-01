use super::*;

impl Store {
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
}
