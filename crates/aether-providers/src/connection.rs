//! Explicit connection preparation, without storage, credentials services or rotation.

use std::collections::HashSet;

use aether_support::connection::RESERVED_HEADERS;
use aether_support::{Connection, KitError, Protocol};
use zeroize::Zeroizing;

use crate::{Model, ModelType, api_base};

/// One endpoint's raw configuration and this operation's fixed key order.
/// Contains plaintext secrets; intentionally has no Debug or serialization implementation.
pub struct EndpointSnapshot {
    /// Optional configuration identity, not an authorization grant.
    pub endpoint_id: Option<String>,
    /// Raw configured API address.
    pub base_url: String,
    /// Wire protocol.
    pub protocol: Protocol,
    /// Fixed key attempt order; empty means anonymous.
    pub api_keys: Vec<Zeroizing<String>>,
    /// Sensitive custom request headers.
    pub headers: Vec<(String, String)>,
    /// Optional image generation API address override.
    pub image_base_url: Option<String>,
    /// Optional image editing API address override.
    pub image_edit_base_url: Option<String>,
}

/// Endpoint configuration with optional already-resolved model metadata.
pub struct ConnectionSnapshot {
    /// Endpoint configuration.
    pub endpoint: EndpointSnapshot,
    /// Effective metadata, including an explicitly disabled reasoning capability.
    pub model: Option<Model>,
}

/// Validates and normalizes one explicit snapshot without reading or rotating configuration.
/// Actual HTTP consumers must still enforce their address/DNS policy.
///
/// # Errors
/// Invalid URL, header or model fields return `bad_request`; an empty model address
/// returns `needs_config{missing_url}`.
pub fn prepare(snapshot: ConnectionSnapshot) -> Result<Connection, KitError> {
    let ConnectionSnapshot {
        mut endpoint,
        model,
    } = snapshot;
    validate_endpoint(&endpoint)?;
    for (name, _) in &mut endpoint.headers {
        if name.len() != name.trim().len() {
            *name = name.trim().to_owned();
        }
    }
    if let Some(model) = &model {
        validate_model(&endpoint, model)?;
    }
    let (
        model_id,
        context_window,
        max_output,
        supports_tools,
        reasoning,
        native_web_search,
        can_vision,
        pricing,
    ) = model.map_or(
        (None, None, None, None, None, false, false, None),
        |model| {
            (
                Some(model.id),
                model.context_window,
                model.max_output,
                model.supports_tools,
                model.reasoning,
                model.native_web_search,
                model.can_vision,
                model.pricing,
            )
        },
    );
    Ok(Connection {
        base_url: api_base(&endpoint.base_url),
        protocol: endpoint.protocol,
        api_keys: endpoint.api_keys,
        model_id,
        endpoint_id: endpoint.endpoint_id,
        context_window,
        max_output,
        supports_tools,
        reasoning,
        native_web_search,
        can_vision,
        pricing,
        headers: endpoint.headers,
        image_base_url: normalize_override(endpoint.image_base_url),
        image_edit_base_url: normalize_override(endpoint.image_edit_base_url),
    })
}

pub(crate) fn validate_model(endpoint: &EndpointSnapshot, model: &Model) -> Result<(), KitError> {
    if endpoint.base_url.trim().is_empty() {
        return Err(KitError::needs_config("missing_url"));
    }
    if model.id.is_empty() {
        return Err(KitError::bad_request("models", "empty_id"));
    }
    if model.context_window == Some(0) {
        return Err(KitError::bad_request("contextWindow", "invalid"));
    }
    if model.max_output == Some(0) {
        return Err(KitError::bad_request("maxOutput", "invalid"));
    }
    if model.model_type == ModelType::Chat
        && let (Some(window), Some(output)) = (model.context_window, model.max_output)
        && output >= window
    {
        return Err(KitError::bad_request("maxOutput", "exceeds_context"));
    }
    if model.native_web_search
        && (model.model_type != ModelType::Chat || endpoint.protocol == Protocol::OpenaiChat)
    {
        return Err(KitError::bad_request(
            "nativeWebSearch",
            "unsupported_protocol",
        ));
    }
    if let Some(reasoning) = &model.reasoning {
        let reason = if model.model_type == ModelType::Chat {
            reasoning
                .validate(endpoint.protocol, model.max_output)
                .err()
        } else {
            Some("invalid_efforts")
        };
        if let Some(reason) = reason {
            return Err(KitError::bad_request("reasoning", reason));
        }
    }
    if model
        .pricing
        .as_ref()
        .is_some_and(|pricing| model.model_type != ModelType::Chat || !pricing.is_valid())
    {
        return Err(KitError::bad_request("pricing", "invalid"));
    }
    Ok(())
}

fn normalize_override(value: Option<String>) -> Option<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .as_deref()
        .map(api_base)
}

pub(crate) fn validate_endpoint(endpoint: &EndpointSnapshot) -> Result<(), KitError> {
    validate_url("baseUrl", &endpoint.base_url)?;
    for (field, value) in [
        ("imageBaseUrl", &endpoint.image_base_url),
        ("imageEditBaseUrl", &endpoint.image_edit_base_url),
    ] {
        if let Some(value) = value {
            validate_url(field, value)?;
        }
    }
    validate_headers(
        endpoint
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    )?;
    for key in &endpoint.api_keys {
        if key.trim().is_empty() {
            return Err(KitError::bad_request("apiKey", "empty"));
        }
        if key.len() > 4096 {
            return Err(KitError::bad_request("apiKey", "too_long"));
        }
        if reqwest::header::HeaderValue::from_str(key.as_str()).is_err() {
            return Err(KitError::bad_request("apiKey", "invalid"));
        }
    }
    Ok(())
}

pub(crate) fn validate_url(field: &str, value: &str) -> Result<(), KitError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > 2048
        || reqwest::Url::parse(value).is_err()
        || !(value.starts_with("http://") || value.starts_with("https://"))
    {
        return Err(KitError::bad_request(field, "invalid_url"));
    }
    Ok(())
}

pub(crate) fn validate_headers<'a>(
    headers: impl ExactSizeIterator<Item = (&'a str, &'a str)>,
) -> Result<(), KitError> {
    if headers.len() > 20 {
        return Err(KitError::bad_request("headers", "too_many"));
    }
    let mut names = HashSet::new();
    for (name, value) in headers {
        let name = name.trim().to_ascii_lowercase();
        let token = (1..=100).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b));
        if !token
            || !names.insert(name.clone())
            || value.chars().count() > 2048
            || value.contains(['\r', '\n'])
        {
            return Err(KitError::bad_request("headers", "invalid"));
        }
        if RESERVED_HEADERS.contains(&name.as_str()) {
            return Err(KitError::bad_request("headers", "reserved"));
        }
    }
    Ok(())
}
