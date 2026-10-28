//! Public search configuration and result data, without execution credentials.
use serde::{Deserialize, Serialize};

/// Search queries and URL reads have independent provider configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchCap {
    /// Retrieve sources for a query.
    Search,
    /// Read a specific webpage.
    Fetch,
}
impl SearchCap {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Fetch => "fetch",
        }
    }
}
/// Provider metadata and masked persisted state; never contains a clear credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProvider {
    /// Provider identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Provider description.
    pub description: String,
    /// Provider home page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_website: Option<String>,
    /// Credential management page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_website: Option<String>,
    /// required, optional, or none.
    pub key_mode: String,
    /// Whether the credential is a Basic-auth password.
    pub basic_auth: bool,
    /// Default API base; absent for direct fetching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_base_url: Option<String>,
    /// Effective API base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Independently encrypted credential records; never plaintext.
    pub keys: Vec<SearchKey>,
    /// Basic-auth username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basic_auth_username: Option<String>,
    /// Whether required credentials are readable.
    pub ready: bool,
}
/// Settings of one capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSettings {
    /// Capability being configured.
    pub capability: SearchCap,
    /// Default provider identifier.
    pub default_provider: String,
    /// Global search policy shared by both capabilities.
    pub preferences: SearchPreferences,
    /// Supported providers with their independent stored states.
    pub providers: Vec<SearchProvider>,
}
/// Partial provider address and username update.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchSave {
    /// Capability partition.
    pub capability: SearchCap,
    /// Provider identifier.
    pub provider: String,
    /// Optional base update; empty restores the default.
    pub base_url: Option<String>,
    /// Three-state Basic username update.
    pub basic_auth_username: Option<String>,
}
/// Safe credential list record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchKey {
    /// Opaque credential identifier.
    pub id: String,
    /// Display-only secret mask.
    pub masked: String,
}
/// Snippet compression policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    /// Preserve response-budget-bounded snippets.
    None,
    /// Truncate snippets at a Unicode scalar boundary.
    Cutoff,
}
/// Global preferences persisted as one validated JSON document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPreferences {
    /// Result ceiling, 1–50.
    pub max_results: u8,
    /// Snippet compression.
    pub compression: Compression,
    /// Unicode character ceiling, 100–20000.
    pub cutoff_limit: usize,
    /// Domain and URL-glob exclusion rules.
    pub blacklist: Vec<String>,
    /// Prefer explicitly supported model-native tools.
    pub model_tools_preferred: bool,
}
impl Default for SearchPreferences {
    fn default() -> Self {
        Self {
            max_results: 5,
            compression: Compression::None,
            cutoff_limit: 2000,
            blacklist: Vec::new(),
            model_tools_preferred: false,
        }
    }
}
/// One publicly linked source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Source title.
    pub title: String,
    /// HTTP(S) source URL.
    pub url: String,
    /// Bounded reference excerpt.
    pub snippet: String,
}
/// Results from a selected provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Actual query transmitted upstream.
    pub query: String,
    /// Selected provider identifier.
    pub provider: String,
    /// Ordered, deduplicated sources.
    pub hits: Vec<SearchHit>,
}
/// Readable webpage output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    /// Page title.
    pub title: String,
    /// Page URL.
    pub url: String,
    /// Bounded plain text/Markdown body.
    pub content: String,
}
/// Successful saved-configuration check.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CheckResult {
    /// Sources returned, or one successfully read page.
    pub count: usize,
}
