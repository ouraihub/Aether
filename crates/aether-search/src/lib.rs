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
use std::{collections::HashMap, path::Path, sync::Mutex, time::Duration};
use zeroize::Zeroizing;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "CREATE TABLE search_settings(capability TEXT PRIMARY KEY,provider TEXT NOT NULL); CREATE TABLE search_providers(capability TEXT NOT NULL,provider TEXT NOT NULL,base_url TEXT,basic_username TEXT,PRIMARY KEY(capability,provider)); CREATE TABLE search_keys(id TEXT PRIMARY KEY,capability TEXT NOT NULL,provider TEXT NOT NULL,key_ciphertext BLOB NOT NULL,position INTEGER NOT NULL); CREATE TABLE search_preferences(id INTEGER PRIMARY KEY CHECK(id=1),json TEXT NOT NULL);",
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
enum Output {
    Hits(Vec<SearchHit>),
    Page(SearchPage),
}
fn normalized_rule(rule: &str) -> Result<(String, bool), KitError> {
    let invalid = || KitError::bad_request("blacklist", "invalid_rule");
    if rule.is_empty()
        || rule.chars().count() > 256
        || rule.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid());
    }
    let (scheme, rest) = match rule.split_once("://") {
        Some((scheme, rest))
            if matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "*") =>
        {
            (Some(scheme.to_ascii_lowercase()), rest)
        }
        Some(_) => return Err(invalid()),
        None => (None, rule),
    };
    let end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..end];
    if scheme.is_none() && end != rest.len() {
        return Err(invalid());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if scheme.is_some() && port.parse::<u16>().is_ok_and(|p| p != 0) => {
            (host, Some(port))
        }
        Some(_) => return Err(invalid()),
        None => (authority, None),
    };
    let host = host.trim_end_matches('.');
    if host.len() > 253
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'*' | b'?'))
        })
    {
        return Err(invalid());
    }
    let host = host.to_ascii_lowercase();
    if let Some(scheme) = scheme {
        if rest[end..].contains(['\\', '[', ']', '{', '}', '^', '|', '#']) {
            return Err(invalid());
        }
        Ok((
            format!(
                "{scheme}://{host}{}{}",
                port.map(|p| format!(":{p}")).unwrap_or_default(),
                if end == rest.len() { "/" } else { &rest[end..] }
            ),
            true,
        ))
    } else {
        Ok((host, false))
    }
}
fn validate_preferences(prefs: &SearchPreferences) -> Result<(), KitError> {
    if !(1..=50).contains(&prefs.max_results) {
        return Err(KitError::bad_request("maxResults", "out_of_range"));
    }
    if !(100..=20000).contains(&prefs.cutoff_limit) {
        return Err(KitError::bad_request("cutoffLimit", "out_of_range"));
    }
    if prefs.blacklist.len() > 32 {
        return Err(KitError::bad_request("blacklist", "too_many"));
    }
    for rule in &prefs.blacklist {
        normalized_rule(rule)?;
    }
    Ok(())
}
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            retry += 1;
            j = retry;
            i = s + 1;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}
fn blocked_url(url: &reqwest::Url, prefs: &SearchPreferences) -> bool {
    let host = url.host_str().unwrap_or_default().trim_end_matches('.');
    let mut normalized = url.clone();
    let _ = normalized.set_host(Some(host));
    prefs.blacklist.iter().any(|raw| {
        let Ok((rule, full)) = normalized_rule(raw) else {
            return true;
        };
        if full {
            glob_match(&rule, normalized.as_str())
        } else if rule.contains(['*', '?']) {
            glob_match(&rule, host)
        } else {
            host == rule || host.strip_suffix(&rule).is_some_and(|s| s.ends_with('.'))
        }
    })
}
fn check_blacklist(url: &str, prefs: &SearchPreferences) -> Result<(), KitError> {
    let url = reqwest::Url::parse(url).map_err(|_| KitError::bad_request("url", "invalid"))?;
    if blocked_url(&url, prefs) {
        return Err(KitError::bad_request("url", "blacklisted"));
    }
    Ok(())
}
fn cutoff(text: &mut String, prefs: &SearchPreferences) {
    if prefs.compression == Compression::Cutoff
        && let Some((end, _)) = text.char_indices().nth(prefs.cutoff_limit)
    {
        text.truncate(end);
    }
}
fn process_hits(hits: &mut Vec<SearchHit>, prefs: &SearchPreferences) {
    hits.retain(|hit| reqwest::Url::parse(&hit.url).is_ok_and(|url| !blocked_url(&url, prefs)));
    for hit in hits {
        cutoff(&mut hit.snippet, prefs);
    }
}
type Row = (Option<String>, Option<String>);
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
    fn runtime(
        &self,
        cap: SearchCap,
        provider: Option<&str>,
    ) -> Result<(Runtime, Vec<Zeroizing<String>>, SearchPreferences), KitError> {
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
        Ok((
            Runtime {
                provider: provider.into(),
                base: base
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| base0.into()),
                key: None,
                user,
                max: prefs.max_results,
            },
            keys,
            prefs,
        ))
    }
    async fn execute(
        &self,
        cap: SearchCap,
        provider: Option<&str>,
        input: &str,
    ) -> Result<(String, Output), KitError> {
        let (mut runtime, keys, prefs) = self.runtime(cap, provider)?;
        if cap == SearchCap::Fetch {
            check_blacklist(input, &prefs)?;
        }
        let duration = Duration::from_secs(match cap {
            SearchCap::Fetch => 20,
            SearchCap::Search if runtime.provider == "exa-mcp" => 25,
            SearchCap::Search => 15,
        });
        tokio::time::timeout(duration, async {
            let attempts = keys.len().max(1);
            let mut keys = keys.into_iter();
            for attempt in 0..attempts {
                runtime.key = keys.next();
                let result = match cap {
                    SearchCap::Search => protocol::search(&self.policy, &runtime, input)
                        .await
                        .map(Output::Hits),
                    SearchCap::Fetch => protocol::fetch(&self.policy, &runtime, input, &prefs)
                        .await
                        .map(Output::Page),
                };
                match result {
                    Ok(mut out) => {
                        match &mut out {
                            Output::Hits(hits) => process_hits(hits, &prefs),
                            Output::Page(page) => {
                                check_blacklist(&page.url, &prefs)?;
                                cutoff(&mut page.content, &prefs);
                            }
                        }
                        return Ok((runtime.provider, out));
                    }
                    Err(e)
                        if attempt + 1 < attempts
                            && matches!(e.details().status(), Some(401 | 403 | 429)) => {}
                    Err(e) => return Err(e),
                }
            }
            Err(KitError::internal(std::io::Error::other(
                "search attempts exhausted without result",
            )))
        })
        .await
        .map_err(|_| KitError::new(KitErrorCode::Timeout, "search execution timed out"))?
    }
    /// Searches using round-robin credentials and global result processing.
    /// # Errors
    /// Reports invalid input, configuration, network and upstream failures.
    pub async fn search(&self, query: &str) -> Result<SearchResult, KitError> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 4096 {
            return Err(KitError::bad_request("query", "invalid"));
        }
        let (provider, Output::Hits(hits)) = self.execute(SearchCap::Search, None, query).await?
        else {
            return Err(KitError::internal(std::io::Error::other(
                "unexpected search output",
            )));
        };
        Ok(SearchResult {
            query: query.into(),
            provider,
            hits,
        })
    }
    /// Reads through the selected provider with the same bounded credential retries.
    /// # Errors
    /// Reports unsafe URLs, missing configuration and upstream failures.
    pub async fn fetch(&self, url: &str) -> Result<SearchPage, KitError> {
        if url.len() > 8192 {
            return Err(KitError::bad_request("url", "too_long"));
        }
        let (_, Output::Page(page)) = self.execute(SearchCap::Fetch, None, url.trim()).await?
        else {
            return Err(KitError::internal(std::io::Error::other(
                "unexpected fetch output",
            )));
        };
        Ok(page)
    }
    /// Checks saved settings through the same executor used by normal requests.
    /// # Errors
    /// Reports configuration, upstream failures or no-result searches.
    pub async fn check(&self, cap: SearchCap, provider: &str) -> Result<CheckResult, KitError> {
        let (_, out) = self
            .execute(
                cap,
                Some(provider),
                if cap == SearchCap::Search {
                    "web search"
                } else {
                    "https://example.com"
                },
            )
            .await?;
        let count = match out {
            Output::Hits(hits) => hits.len(),
            Output::Page(_) => 1,
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
