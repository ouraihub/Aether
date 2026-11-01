use super::*;

pub(super) fn validate(input: &ProviderSave) -> Result<(), KitError> {
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
pub(super) const REASONING_OFF: &str = "none";

/// The one read path of `models.reasoning`: NULL = not set (inferred for chat models),
/// `'none'` = turned off, otherwise the stored config (docs/41 §2).
pub(super) fn read_reasoning(
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

pub(super) fn read_pricing(stored: Option<&str>) -> Result<Option<ModelPricing>, KitError> {
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

pub(super) fn write_endpoint(
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

pub(super) fn list(db: &Db) -> Result<Vec<Provider>, KitError> {
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

pub(super) fn parse_headers(json: &str) -> Result<BTreeMap<String, String>, KitError> {
    serde_json::from_str(json).map_err(|_| corrupt("headers"))
}

/// Ciphertexts of `endpoint_keys` rows matching `filter`, to `forget` after they are deleted.
pub(super) fn ciphertexts(
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

pub(super) fn forget_all(stale: &[Vec<u8>]) -> Result<(), KitError> {
    stale.iter().try_for_each(|sealed| secret::forget(sealed))
}

pub(super) const fn protocol_str(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::AnthropicMessages => "anthropic-messages",
        Protocol::OpenaiChat => "openai-chat",
        Protocol::OpenaiResponses => "openai-responses",
    }
}

pub(super) fn parse_protocol(text: &str) -> Result<Protocol, KitError> {
    match text {
        "anthropic-messages" => Ok(Protocol::AnthropicMessages),
        "openai-chat" => Ok(Protocol::OpenaiChat),
        "openai-responses" => Ok(Protocol::OpenaiResponses),
        _ => Err(corrupt("protocol")),
    }
}

pub(super) fn corrupt(column: &str) -> KitError {
    KitError::internal(std::io::Error::other(format!(
        "providers.db: bad {column} value"
    )))
}
