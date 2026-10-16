//! Fetching an endpoint's model list (docs/11 §5). Never writes the database.

use std::net::IpAddr;
use std::time::Duration;

use aether_support::error::upstream_excerpt;
use aether_support::net::{self, AddressPolicy};
use aether_support::{KitError, Protocol};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    Model, ProbeError, ProbeErrorKind, ProbeResult, ProbeTarget, ProbedModel, Store, TypeSource,
    api_base, infer, infer_reasoning,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY: usize = 4 * 1024 * 1024;

type Outcome = Result<Vec<ProbedModel>, ProbeError>;

impl Store {
    /// `GET <api_base>/models` for a saved endpoint or an unsaved draft. Upstream and
    /// network failures are reported in [`ProbeResult::error`]; an unauthorized or
    /// rate-limited key falls through to the next enabled key (docs/34 §3.4).
    ///
    /// # Errors
    /// `cancelled` when `cancel` fires; `not_found{entity:"endpoint"|"key"}` or
    /// `needs_config{reason:"key_unreadable"}` for stored keys; `bad_request`
    /// for an unusable URL (field `url`) or key (field `apiKey`); `internal`.
    pub async fn probe(
        &self,
        target: ProbeTarget,
        cancel: &CancellationToken,
    ) -> Result<ProbeResult, KitError> {
        let (base_url, protocol, keys) = match target {
            ProbeTarget::Saved { endpoint_id } => self.endpoint(&endpoint_id)?,
            // The address may be edited and unsaved while the keys stay stored (docs/11 §4).
            ProbeTarget::Draft {
                base_url,
                protocol,
                endpoint_id,
                key_id,
            } => {
                let keys = match (endpoint_id, key_id) {
                    (Some(endpoint), Some(key)) => vec![self.endpoint_key(&endpoint, &key)?],
                    (Some(endpoint), None) => self.endpoint(&endpoint)?.2,
                    (None, Some(_)) => return Err(KitError::not_found("key")),
                    (None, None) => Vec::new(),
                };
                (base_url, protocol, keys)
            }
        };
        let probe_url = format!("{}/models", api_base(&base_url));
        let known: Vec<&str> = keys.iter().map(|key| key.as_str()).collect();
        let attempts = keys.len().max(1);
        let mut outcome = Ok(Vec::new());
        for attempt in 0..attempts {
            outcome = cancel
                .run_until_cancelled(fetch(
                    &probe_url,
                    &base_url,
                    protocol,
                    keys.get(attempt),
                    &known,
                ))
                .await
                .ok_or_else(KitError::cancelled)??;
            let next = matches!(&outcome, Err(e) if matches!(e.kind, ProbeErrorKind::Unauthorized | ProbeErrorKind::RateLimited));
            if !next {
                break;
            }
        }
        let (models, error) = match outcome {
            Ok(models) => (models, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        Ok(ProbeResult {
            probe_url,
            models,
            error,
        })
    }
}

async fn fetch(
    probe_url: &str,
    base_url: &str,
    protocol: Protocol,
    key: Option<&Zeroizing<String>>,
    known: &[&str],
) -> Result<Outcome, KitError> {
    let policy = AddressPolicy::default();
    let url = match net::check_url(probe_url, &policy) {
        Ok(url) => url,
        Err(e) if e.details().reason() == Some("blocked_address") => {
            return Ok(Err(failure(ProbeErrorKind::BlockedAddress)));
        }
        Err(e) => return Err(e),
    };
    let host = url.host_str().unwrap_or_default();
    let ip: Option<IpAddr> = host.trim_matches(['[', ']']).parse().ok();
    let loopback = ip.map_or(host == "localhost", |ip| ip.to_canonical().is_loopback());
    if key.is_none() && !loopback {
        return Ok(Err(failure(ProbeErrorKind::MissingKey)));
    }

    let mut request = net::client(policy, CONNECT_TIMEOUT)?
        .get(url)
        .timeout(TOTAL_TIMEOUT);
    if protocol == Protocol::AnthropicMessages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    if let Some(key) = key {
        // Many relays only read Bearer, so anthropic gets both headers.
        request = request.header(
            AUTHORIZATION,
            sensitive(&Zeroizing::new(format!("Bearer {}", key.as_str())))?,
        );
        if protocol == Protocol::AnthropicMessages {
            request = request.header("x-api-key", sensitive(key)?);
        }
    }
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(e) => return Ok(Err(transport(&e, ip.is_some()))),
    };

    let status = response.status().as_u16();
    if !response.status().is_success() {
        // The error body only adds a message; failing to read it keeps the status.
        let body = read_capped(&mut response).await.ok().flatten();
        let message = body
            .map(|b| upstream_excerpt(&String::from_utf8_lossy(&b), known))
            .filter(|m| !m.is_empty());
        let (kind, hint) = match status {
            401 | 403 => (
                ProbeErrorKind::Unauthorized,
                Some("providers.hint.check_key"),
            ),
            404 => (
                ProbeErrorKind::NotFound,
                (!base_url.contains("/v1")).then_some("providers.hint.missing_v1"),
            ),
            429 => (ProbeErrorKind::RateLimited, None),
            _ => (ProbeErrorKind::Status, None),
        };
        return Ok(Err(ProbeError {
            kind,
            status: Some(status),
            message,
            hint: hint.map(str::to_owned),
        }));
    }
    let body = match read_capped(&mut response).await {
        Ok(Some(body)) => body,
        Ok(None) => return Ok(Err(failure(ProbeErrorKind::InvalidBody))),
        Err(e) => return Ok(Err(transport(&e, ip.is_some()))),
    };
    Ok(parse_ids(&body)
        .filter(|ids| !ids.is_empty())
        .map(|ids| ids.into_iter().map(|id| probed(id, protocol)).collect())
        .ok_or_else(|| failure(ProbeErrorKind::InvalidBody)))
}

/// The whole body, or `None` once it exceeds [`MAX_BODY`].
async fn read_capped(response: &mut reqwest::Response) -> Result<Option<Vec<u8>>, reqwest::Error> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_BODY {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

/// Ids from `{data:[{id}]}`, `{models:[{id}|{name}]}` or `[string]`, sorted and deduplicated.
fn parse_ids(body: &[u8]) -> Option<Vec<String>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let items = match value.as_object() {
        Some(map) => map.get("data").or_else(|| map.get("models"))?,
        None => &value,
    }
    .as_array()?;
    let mut ids = items
        .iter()
        .map(|item| {
            item.as_str()
                .or_else(|| {
                    let map = item.as_object()?;
                    map.get("id").or_else(|| map.get("name"))?.as_str()
                })
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    ids.sort_unstable();
    ids.dedup();
    Some(ids)
}

fn probed(id: String, protocol: Protocol) -> ProbedModel {
    let inferred = infer(&id);
    let reasoning = (inferred.model_type == crate::ModelType::Chat)
        .then(|| infer_reasoning(&id, protocol, inferred.max_output))
        .flatten();
    ProbedModel {
        model: Model {
            id,
            model_type: inferred.model_type,
            type_source: TypeSource::Inferred,
            context_window: inferred.context_window,
            max_output: inferred.max_output,
            can_vision: inferred.can_vision,
            supports_tools: inferred.supports_tools,
            reasoning,
            native_web_search: false,
        },
        group: inferred.group,
    }
}

fn sensitive(value: &str) -> Result<HeaderValue, KitError> {
    let mut header = HeaderValue::from_str(value)
        .map_err(|_| KitError::bad_request("apiKey", "invalid_header"))?;
    header.set_sensitive(true);
    Ok(header)
}

fn transport(error: &reqwest::Error, ip_host: bool) -> ProbeError {
    if net::is_blocked(error) {
        failure(ProbeErrorKind::BlockedAddress)
    } else if error.is_timeout() {
        failure(ProbeErrorKind::Timeout)
    } else {
        ProbeError {
            hint: ip_host.then(|| "providers.hint.check_network".to_owned()),
            ..failure(ProbeErrorKind::Connect)
        }
    }
}

const fn failure(kind: ProbeErrorKind) -> ProbeError {
    ProbeError {
        kind,
        status: None,
        message: None,
        hint: None,
    }
}
