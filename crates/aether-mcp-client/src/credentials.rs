//! Per-server credentials, sealed at rest as JSON (docs/13 §2): request headers and query
//! parameters for http servers, environment variables for stdio servers.

use std::collections::BTreeMap;

use aether_support::KitError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

/// `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
const MAX_ENV_KEY_LEN: usize = 128;

// ponytail: the parsed maps are plain Strings, not zeroized; only the sealed JSON is.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Secret {
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) query: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
}

impl Secret {
    pub(crate) fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.query.is_empty() && self.env.is_empty()
    }

    /// Names only, for display.
    pub(crate) fn keys(&self) -> Vec<String> {
        self.headers
            .keys()
            .chain(self.query.keys())
            .chain(self.env.keys())
            .cloned()
            .collect()
    }

    /// Plaintext values, replaced in upstream error excerpts and logs.
    pub(crate) fn values(&self) -> Vec<String> {
        self.headers
            .values()
            .chain(self.query.values())
            .chain(self.env.values())
            .cloned()
            .collect()
    }

    /// Validates header names/values and query names.
    pub(crate) fn header_map(&self) -> Result<HeaderMap, KitError> {
        if self.query.keys().any(String::is_empty) {
            return Err(KitError::bad_request("secret", "query"));
        }
        let mut map = HeaderMap::new();
        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| KitError::bad_request("secret", "header"))?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| KitError::bad_request("secret", "header"))?;
            value.set_sensitive(true);
            map.insert(name, value);
        }
        Ok(map)
    }

    /// Validates environment variable names; a NUL in a value cannot be passed to a process.
    pub(crate) fn check_env(&self) -> Result<(), KitError> {
        let valid = |name: &str| {
            let mut bytes = name.bytes();
            name.len() <= MAX_ENV_KEY_LEN
                && bytes
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        };
        if !self.env.keys().all(|name| valid(name)) {
            return Err(KitError::bad_request("secret", "env_key"));
        }
        if self.env.values().any(|value| value.contains('\0')) {
            return Err(KitError::bad_request("secret", "env_value"));
        }
        Ok(())
    }
}
