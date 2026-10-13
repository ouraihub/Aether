//! Independent search provider settings, encrypted credentials and bounded execution.
mod protocol;
pub use aether_support::net::AddressPolicy;
use aether_support::{
    KitError, KitErrorCode,
    db::{self, Migration},
    secret,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};
use zeroize::Zeroizing;

const DEFAULT_RESULTS: u8 = 5;
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "CREATE TABLE search_settings(capability TEXT PRIMARY KEY,provider TEXT NOT NULL,max_results INTEGER NOT NULL DEFAULT 5); CREATE TABLE search_providers(capability TEXT NOT NULL,provider TEXT NOT NULL,base_url TEXT,api_key BLOB,basic_username TEXT,updated_at INTEGER NOT NULL,PRIMARY KEY(capability,provider));",
}];
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
    fn key(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Fetch => "fetch",
        }
    }
}
struct Spec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    website: &'static str,
    key_site: Option<&'static str>,
    basic: bool,
    search: Option<(&'static str, &'static str)>,
    fetch: Option<(&'static str, &'static str)>,
}
const PROVIDERS: &[Spec] = &[
    Spec {
        id: "zhipu",
        name: "智谱 Zhipu",
        description: "GLM Web Search",
        website: "https://docs.bigmodel.cn/cn/guide/tools/web-search",
        key_site: Some(
            "https://www.bigmodel.cn/invite?icode=X2d48dEyAbBOUJCLpZmYJUjPr3uHog9F4g5tjuOUqno%3D",
        ),
        basic: false,
        search: Some((
            "https://open.bigmodel.cn/api/paas/v4/web_search",
            "required",
        )),
        fetch: None,
    },
    Spec {
        id: "tavily",
        name: "Tavily",
        description: "Search optimized for LLMs",
        website: "https://tavily.com",
        key_site: Some("https://app.tavily.com/home"),
        basic: false,
        search: Some(("https://api.tavily.com", "required")),
        fetch: None,
    },
    Spec {
        id: "searxng",
        name: "SearXNG",
        description: "Self-hosted meta search",
        website: "https://docs.searxng.org",
        key_site: None,
        basic: true,
        search: Some(("http://localhost:8080", "none")),
        fetch: None,
    },
    Spec {
        id: "exa",
        name: "Exa",
        description: "Neural search API",
        website: "https://exa.ai",
        key_site: Some("https://dashboard.exa.ai/api-keys"),
        basic: false,
        search: Some(("https://api.exa.ai", "required")),
        fetch: None,
    },
    Spec {
        id: "exa-mcp",
        name: "Exa MCP",
        description: "MCP search; anonymous access subject to provider limits",
        website: "https://exa.ai",
        key_site: Some("https://dashboard.exa.ai/api-keys"),
        basic: false,
        search: Some(("https://mcp.exa.ai/mcp", "optional")),
        fetch: None,
    },
    Spec {
        id: "bocha",
        name: "博查 Bocha",
        description: "Chinese web search",
        website: "https://bochaai.com",
        key_site: Some("https://open.bochaai.com/overview"),
        basic: false,
        search: Some(("https://api.bochaai.com", "required")),
        fetch: None,
    },
    Spec {
        id: "querit",
        name: "Querit",
        description: "Search and page content",
        website: "https://querit.ai",
        key_site: Some("https://www.querit.ai/en/dashboard/api-keys"),
        basic: false,
        search: Some(("https://api.querit.ai", "required")),
        fetch: Some(("https://api.querit.ai", "required")),
    },
    Spec {
        id: "brave",
        name: "Brave Search",
        description: "Independent search index",
        website: "https://brave.com/search/api/",
        key_site: Some("https://api-dashboard.search.brave.com/app/keys"),
        basic: false,
        search: Some(("https://api.search.brave.com", "required")),
        fetch: None,
    },
    Spec {
        id: "parallel",
        name: "Parallel",
        description: "Search with relevant page excerpts",
        website: "https://parallel.ai",
        key_site: Some("https://platform.parallel.ai"),
        basic: false,
        search: Some(("https://api.parallel.ai", "required")),
        fetch: None,
    },
    Spec {
        id: "jina",
        name: "Jina",
        description: "Reader search and webpage extraction",
        website: "https://jina.ai/reader",
        key_site: Some("https://jina.ai"),
        basic: false,
        search: Some(("https://s.jina.ai", "required")),
        fetch: Some(("https://r.jina.ai", "optional")),
    },
    Spec {
        id: "firecrawl",
        name: "Firecrawl",
        description: "Web search and Markdown extraction",
        website: "https://firecrawl.dev",
        key_site: Some("https://firecrawl.dev/app/api-keys"),
        basic: false,
        search: Some(("https://api.firecrawl.dev", "optional")),
        fetch: Some(("https://api.firecrawl.dev", "optional")),
    },
    Spec {
        id: "fetch",
        name: "Direct fetch",
        description: "Read static public HTML without a key",
        website: "",
        key_site: None,
        basic: false,
        search: None,
        fetch: Some(("", "none")),
    },
];
fn lookup(
    cap: SearchCap,
    id: &str,
) -> Result<(&'static Spec, &'static str, &'static str), KitError> {
    PROVIDERS
        .iter()
        .find_map(|spec| {
            let config = match cap {
                SearchCap::Search => spec.search,
                SearchCap::Fetch => spec.fetch,
            };
            config
                .filter(|_| spec.id == id)
                .map(|(base, mode)| (spec, base, mode))
        })
        .ok_or_else(|| KitError::bad_request("provider", "unsupported"))
}
fn default_provider(cap: SearchCap) -> &'static str {
    match cap {
        SearchCap::Search => "exa-mcp",
        SearchCap::Fetch => "jina",
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
    /// Masked stored credential or unreadable state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_masked: Option<String>,
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
    /// Search-only result ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u8>,
    /// Supported providers with their independent stored states.
    pub providers: Vec<SearchProvider>,
}
/// Partial update; omitted key keeps, empty key clears, nonempty key replaces.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchSave {
    /// Capability partition.
    pub capability: SearchCap,
    /// Provider identifier.
    pub provider: String,
    /// Optional base update; empty restores the default.
    pub base_url: Option<String>,
    /// Three-state credential update.
    pub api_key: Option<String>,
    /// Three-state Basic username update.
    pub basic_auth_username: Option<String>,
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
type Row = (Option<String>, Option<Vec<u8>>, Option<String>);
struct Runtime {
    provider: String,
    base: String,
    key: Option<Zeroizing<String>>,
    user: Option<String>,
    max: u8,
}
/// SQLite configuration store; locks are released before network operations.
pub struct Store {
    db: Mutex<Connection>,
    policy: AddressPolicy,
}
impl Store {
    /// Opens and migrates an independent search database.
    /// # Errors
    /// Reports SQLite/migration failures without falling back to plaintext credentials.
    pub fn open(path: &Path, policy: AddressPolicy) -> Result<Self, KitError> {
        Ok(Self {
            db: Mutex::new(db::open(path, MIGRATIONS)?),
            policy,
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, KitError> {
        self.db
            .lock()
            .map_err(|_| KitError::internal(std::io::Error::other("search database lock poisoned")))
    }
    fn row(conn: &Connection, cap: SearchCap, id: &str) -> Result<Option<Row>, KitError> {
        Ok(conn.query_row("SELECT base_url,api_key,basic_username FROM search_providers WHERE capability=?1 AND provider=?2",params![cap.key(),id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)
    }
    /// Lists metadata and effective masked settings for one capability.
    /// # Errors
    /// Reports database failures; unreadable credentials stay visible as unavailable.
    pub fn settings(&self, cap: SearchCap) -> Result<SearchSettings, KitError> {
        let db = self.lock()?;
        let (default, max) = db
            .query_row(
                "SELECT provider,max_results FROM search_settings WHERE capability=?1",
                [cap.key()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?)),
            )
            .optional()?
            .unwrap_or((default_provider(cap).into(), DEFAULT_RESULTS));
        let mut providers = Vec::new();
        for spec in PROVIDERS {
            let Some((base0, mode)) = (match cap {
                SearchCap::Search => spec.search,
                SearchCap::Fetch => spec.fetch,
            }) else {
                continue;
            };
            let (base, sealed, user) = Self::row(&db, cap, spec.id)?.unwrap_or((None, None, None));
            let decrypted = sealed.as_deref().map(secret::open).transpose();
            let (mask, readable, has_key) = match decrypted {
                Ok(key) => {
                    let has = key.is_some();
                    (
                        key.as_deref().map(|s| aether_support::mask::mask_secret(s)),
                        true,
                        has,
                    )
                }
                Err(_) => (Some("[unreadable]".into()), false, true),
            };
            let inherited = if spec.id == "exa-mcp" && !has_key {
                Self::row(&db, cap, "exa")?.and_then(|r| r.1).is_some()
            } else {
                false
            };
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
                api_key_masked: mask,
                basic_auth_username: user,
                ready: readable && (mode != "required" || has_key || inherited),
            });
        }
        Ok(SearchSettings {
            capability: cap,
            default_provider: default,
            max_results: (cap == SearchCap::Search).then_some(max),
            providers,
        })
    }
    /// Applies one provider's partial update atomically.
    /// # Errors
    /// Rejects unknown provider, unsafe URL or oversized input; encryption errors preserve existing state.
    pub fn save(&self, input: SearchSave) -> Result<SearchSettings, KitError> {
        let (spec, base0, _) = lookup(input.capability, &input.provider)?;
        if input.base_url.as_ref().is_some_and(|s| s.len() > 8192)
            || input.api_key.as_ref().is_some_and(|s| s.len() > 4096)
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
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let old = Self::row(&tx, input.capability, spec.id)?.unwrap_or((None, None, None));
        let base = match input.base_url {
            _ if base0.is_empty() => None,
            None => old.0,
            Some(value) => {
                let trimmed = value.trim().trim_end_matches('/');
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            }
        };
        let key = if spec.basic || lookup(input.capability, spec.id)?.2 != "none" {
            match input.api_key {
                None => old.1,
                Some(s) if s.is_empty() => None,
                Some(s) => Some(secret::seal(&s)?),
            }
        } else {
            None
        };
        let user = if spec.basic {
            input
                .basic_auth_username
                .map(|s| s.trim().to_owned())
                .or(old.2)
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        tx.execute("INSERT INTO search_providers(capability,provider,base_url,api_key,basic_username,updated_at) VALUES(?1,?2,?3,?4,?5,unixepoch()) ON CONFLICT(capability,provider) DO UPDATE SET base_url=excluded.base_url,api_key=excluded.api_key,basic_username=excluded.basic_username,updated_at=excluded.updated_at",params![input.capability.key(),spec.id,base,key,user])?;
        tx.commit()?;
        drop(db);
        self.settings(input.capability)
    }
    /// Changes the default provider without touching any credentials.
    /// # Errors
    /// Rejects unknown provider or search count outside 1–50.
    pub fn set_default(
        &self,
        cap: SearchCap,
        provider: &str,
        max: Option<u8>,
    ) -> Result<SearchSettings, KitError> {
        lookup(cap, provider)?;
        if let Some(n) = max
            && (cap != SearchCap::Search || !(1..=50).contains(&n))
        {
            return Err(KitError::bad_request("maxResults", "out_of_range"));
        }
        let db = self.lock()?;
        let prior = db
            .query_row(
                "SELECT max_results FROM search_settings WHERE capability=?1",
                [cap.key()],
                |r| r.get::<_, u8>(0),
            )
            .optional()?
            .unwrap_or(DEFAULT_RESULTS);
        db.execute("INSERT INTO search_settings(capability,provider,max_results)VALUES(?1,?2,?3)ON CONFLICT(capability)DO UPDATE SET provider=excluded.provider,max_results=excluded.max_results",params![cap.key(),provider,max.unwrap_or(prior)])?;
        drop(db);
        self.settings(cap)
    }
    /// Returns a stored clear credential only on explicit request.
    /// # Errors
    /// Reports missing/unreadable credentials and unsupported provider.
    pub fn reveal(&self, cap: SearchCap, provider: &str) -> Result<String, KitError> {
        lookup(cap, provider)?;
        let db = self.lock()?;
        let sealed = Self::row(&db, cap, provider)?
            .and_then(|r| r.1)
            .ok_or_else(|| KitError::not_found("apiKey"))?;
        Ok(secret::open(&sealed)?.to_string())
    }
    fn runtime(&self, cap: SearchCap, provider: Option<&str>) -> Result<Runtime, KitError> {
        let settings = self.settings(cap)?;
        let provider = provider.unwrap_or(&settings.default_provider);
        let (_, base0, mode) = lookup(cap, provider)?;
        let db = self.lock()?;
        let (base, mut key, user) = Self::row(&db, cap, provider)?.unwrap_or((None, None, None));
        if provider == "exa-mcp" && key.is_none() {
            key = Self::row(&db, cap, "exa")?.and_then(|r| r.1);
        }
        let key = key.as_deref().map(secret::open).transpose()?;
        if mode == "required" && key.is_none() {
            return Err(KitError::needs_config("api_key"));
        }
        Ok(Runtime {
            provider: provider.into(),
            base: base
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| base0.into()),
            key,
            user,
            max: settings.max_results.unwrap_or(DEFAULT_RESULTS),
        })
    }
    /// Searches the trimmed user query with the currently selected provider.
    /// # Errors
    /// Reports invalid input, configuration, provider, network, timeout and response limits.
    pub async fn search(&self, query: &str) -> Result<SearchResult, KitError> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 4096 {
            return Err(KitError::bad_request("query", "invalid"));
        }
        let runtime = self.runtime(SearchCap::Search, None)?;
        let hits = protocol::search(&self.policy, &runtime, query).await?;
        Ok(SearchResult {
            query: query.into(),
            provider: runtime.provider,
            hits,
        })
    }
    /// Reads a URL through the current fetching provider.
    /// # Errors
    /// Reports unsafe/oversized URL, missing credentials, network and response limits.
    pub async fn fetch(&self, url: &str) -> Result<SearchPage, KitError> {
        if url.len() > 8192 {
            return Err(KitError::bad_request("url", "too_long"));
        }
        let runtime = self.runtime(SearchCap::Fetch, None)?;
        protocol::fetch(&self.policy, &runtime, url.trim()).await
    }
    /// Checks saved settings; no-result searches fail instead of claiming readiness.
    /// # Errors
    /// Reports configuration or real request failure.
    pub async fn check(&self, cap: SearchCap, provider: &str) -> Result<CheckResult, KitError> {
        let runtime = self.runtime(cap, Some(provider))?;
        let count = match cap {
            SearchCap::Search => protocol::search(&self.policy, &runtime, "web search")
                .await?
                .len(),
            SearchCap::Fetch => {
                protocol::fetch(&self.policy, &runtime, "https://example.com").await?;
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
mod tests;
