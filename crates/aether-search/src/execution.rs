//! Explicit per-request configuration and the shared deadline/key-fallback
//! boundary.
use crate::{
    AddressPolicy, SearchCap, SearchHit, SearchPage, SearchPreferences, policy,
    vendor,
};
use aether_support::{KitError, KitErrorCode, net};
use std::time::Duration;
use zeroize::Zeroizing;

/// A single execution's owned inputs; credentials are never serialized or
/// debug-printed.
pub struct ResolvedConfig {
    /// Explicit provider identifier for the requested capability.
    pub provider: String,
    /// Resolved API address; ignored by direct fetching.
    pub base_url: String,
    /// Credential attempt order; an empty vector requests one anonymous
    /// attempt.
    pub api_keys: Vec<Zeroizing<String>>,
    /// Optional SearXNG Basic-auth username.
    pub basic_auth_username: Option<String>,
    /// Validated again at each public execution boundary.
    pub preferences: SearchPreferences,
    /// Host-granted network authorization, never inferred from the request URL.
    pub address_policy: AddressPolicy,
}

pub(crate) enum Output {
    Hits(Vec<SearchHit>),
    Page(SearchPage),
}

/// Borrowed attempt data avoids copying clear credentials into another
/// configuration.
pub(crate) struct Attempt<'a> {
    pub(crate) provider: &'a str,
    pub(crate) base: &'a str,
    pub(crate) key: Option<&'a str>,
    pub(crate) user: Option<&'a str>,
    pub(crate) max: u8,
}

fn validate(config: &ResolvedConfig, cap: SearchCap) -> Result<(), KitError> {
    let (_, _, mode) = vendor::lookup(cap, &config.provider)?;
    policy::validate_preferences(&config.preferences)?;
    if mode == "required" && config.api_keys.is_empty() {
        return Err(KitError::needs_config("keys"));
    }
    if config.provider != "fetch"
        && config
            .api_keys
            .iter()
            .any(|key| {
                key.trim().is_empty() || key.encode_utf16().count() > 4096
            })
    {
        return Err(KitError::bad_request("value", "invalid"));
    }
    if config.provider != "fetch"
        && config
            .basic_auth_username
            .as_ref()
            .is_some_and(|user| user.len() > 200)
    {
        return Err(KitError::bad_request("provider", "too_long"));
    }
    if config.provider != "fetch" {
        if config.base_url.len() > 8192 {
            return Err(KitError::bad_request("provider", "too_long"));
        }
        net::check_url(&config.base_url, &config.address_policy)?;
    }
    Ok(())
}

pub(crate) async fn execute(
    config: &ResolvedConfig,
    cap: SearchCap,
    input: &str,
) -> Result<Output, KitError> {
    validate(config, cap)?;
    let duration = Duration::from_secs(match cap {
        SearchCap::Fetch => 20,
        SearchCap::Search if config.provider == "exa-mcp" => 25,
        SearchCap::Search => 15,
    });
    tokio::time::timeout(duration, async {
        if cap == SearchCap::Fetch {
            net::check_url(input, &config.address_policy)?;
            policy::check_blacklist(input, &config.preferences)?;
        }
        let attempts = if config.provider == "fetch" {
            1
        } else {
            config.api_keys.len().max(1)
        };
        for index in 0..attempts {
            let attempt = Attempt {
                provider: &config.provider,
                base: &config.base_url,
                key: if config.provider == "fetch" {
                    None
                } else {
                    config.api_keys.get(index).map(|key| key.as_str())
                },
                user: if config.provider == "fetch" {
                    None
                } else {
                    config.basic_auth_username.as_deref()
                },
                max: config.preferences.max_results,
            };
            let result = match cap {
                SearchCap::Search => vendor::search(
                    &config.address_policy,
                    &attempt,
                    input,
                )
                .await
                .map(Output::Hits),
                SearchCap::Fetch => vendor::fetch(
                    &config.address_policy,
                    &attempt,
                    input,
                    &config.preferences,
                )
                .await
                .map(Output::Page),
            };
            match result {
                Ok(out) => return Ok(out),
                Err(error)
                    if index + 1 < attempts && net::next_key(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Err(KitError::internal(std::io::Error::other(
            "search attempts exhausted without result",
        )))
    })
    .await
    .map_err(|_| {
        KitError::new(KitErrorCode::Timeout, "search execution timed out")
    })?
}
