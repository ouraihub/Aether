//! Configuration DTOs (docs/11 §3). Requests reject unknown fields.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Protocol;

/// What a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ModelType {
    /// Text chat.
    Chat,
    /// Image generation.
    Image,
    /// Video generation.
    Video,
    /// Text to speech.
    Tts,
    /// Speech to text.
    Stt,
    /// Embeddings.
    Embedding,
    /// Reranking.
    Rerank,
    /// Not recognised.
    Unknown,
}

/// Who decided a model's type; user choices are never overwritten by inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TypeSource {
    /// Inferred from the model id.
    Inferred,
    /// Set by the user.
    User,
}

/// A selected model of an endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct Model {
    /// Upstream model identifier.
    pub id: String,
    /// Intended model capability.
    pub model_type: ModelType,
    /// Origin of the capability declaration.
    pub type_source: TypeSource,
    /// Configured context window in tokens.
    pub context_window: Option<u32>,
    /// Configured maximum output tokens.
    pub max_output: Option<u32>,
    /// Image input support.
    pub can_vision: bool,
    /// Client-side tool support, unknown when absent.
    pub supports_tools: Option<bool>,
    /// User-declared reasoning parameters.
    #[serde(default)]
    pub reasoning: Option<aether_support::ReasoningConfig>,
    /// User-declared support for protocol-native web search.
    #[serde(default)]
    pub native_web_search: bool,
    /// Display name; `None` = show the id.
    #[serde(default)]
    pub name: Option<String>,
    /// Picker group; `None` = ungrouped.
    #[serde(default)]
    pub group_name: Option<String>,
    /// User-declared prices (chat only).
    #[serde(default)]
    pub pricing: Option<aether_support::ModelPricing>,
}

/// Model fields in a save request. `reasoning` is tri-state: omitted keeps the
/// stored value, `null` turns it off, and an object replaces it. `name`, `groupName`
/// and `pricing` replace the stored value (omitted = `null`).
#[derive(Debug, Clone, PartialEq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ModelSave {
    /// Upstream model identifier.
    pub id: String,
    /// Intended model capability.
    pub model_type: ModelType,
    /// Origin of the capability declaration.
    pub type_source: TypeSource,
    /// Configured context window in tokens.
    pub context_window: Option<u32>,
    /// Configured maximum output tokens.
    pub max_output: Option<u32>,
    /// Image input support.
    pub can_vision: bool,
    /// Client-side tool support, unknown when absent.
    pub supports_tools: Option<bool>,
    /// Omit to keep, null to turn off, object to replace reasoning settings.
    #[serde(default, with = "present")]
    #[ts(as = "Option<aether_support::ReasoningConfig>", optional = nullable)]
    pub reasoning: Option<Option<aether_support::ReasoningConfig>>,
    /// User-declared support for protocol-native web search.
    #[serde(default)]
    pub native_web_search: bool,
    /// Display name, at most 80 characters; blank = `None`.
    #[serde(default)]
    pub name: Option<String>,
    /// Picker group, at most 80 characters; blank = `None`.
    #[serde(default)]
    pub group_name: Option<String>,
    /// User-declared prices (chat only).
    #[serde(default)]
    pub pricing: Option<aether_support::ModelPricing>,
}

/// How an endpoint picks among its enabled keys (docs/43 §3 of the host).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KeyStrategy {
    /// Start one key further on every request; failures are not remembered.
    #[default]
    RoundRobin,
    /// Always start at the first key; a key that ran out is skipped until it recovers.
    FillFirst,
}

/// One API key of an endpoint as shown to the UI: never the plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EndpointKey {
    /// Key id.
    pub id: String,
    /// Optional user label, at most 40 characters.
    pub label: String,
    /// Disabled keys are skipped by rotation.
    pub enabled: bool,
    /// First 4 and last 4 characters; `None` when the ciphertext cannot be decrypted.
    pub masked: Option<String>,
    /// `false` when the ciphertext cannot be decrypted (e.g. another Windows user).
    pub readable: bool,
    /// Unix milliseconds until which `FillFirst` skips this key; `None` = usable.
    #[ts(type = "number | null")]
    pub exhausted_until: Option<u64>,
    /// Why the key was set aside, e.g. `HTTP 429`; `None` when usable.
    pub exhausted_reason: Option<String>,
}

/// A stored endpoint as shown to the UI: keys only as masked entries.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Endpoint {
    /// Endpoint id.
    pub id: String,
    /// Short name, unique within the provider.
    pub slug: String,
    /// Base URL as entered; may be empty.
    pub base_url: String,
    /// Wire protocol.
    pub protocol: Protocol,
    /// API keys ordered by position.
    pub keys: Vec<EndpointKey>,
    /// How a request picks among `keys`.
    pub key_strategy: KeyStrategy,
    /// Where model listing is fetched from.
    pub probe_url: String,
    /// Selected models in display order.
    pub models: Vec<Model>,
    /// Image generation base URL; `None` = use `base_url`.
    pub image_base_url: Option<String>,
    /// Image edit (`/images/edits`) base URL; `None` = use `base_url`.
    pub image_edit_base_url: Option<String>,
    /// Custom request headers, applied before system headers.
    pub headers: BTreeMap<String, String>,
}

/// A provider with its endpoints in display order.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Provider {
    /// Provider id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Off = its models leave every picker and cannot run.
    pub enabled: bool,
    /// Free-form Markdown notes about this provider's models.
    pub notes: String,
    /// Host preset this provider belongs to; preset providers cannot be deleted. `None` = user-created.
    pub preset_id: Option<String>,
    /// Endpoints in display order.
    pub endpoints: Vec<Endpoint>,
}

/// One endpoint of a save request. Existing endpoints must carry their `id`.
#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct EndpointSave {
    /// Existing endpoint id; absent = new endpoint.
    #[serde(default)]
    #[ts(optional)]
    pub id: Option<String>,
    /// Short name, 1–40 of `[a-z0-9-]`.
    pub slug: String,
    /// Base URL: empty, or `http(s)://` of at most 2048 bytes.
    pub base_url: String,
    /// Wire protocol.
    pub protocol: Protocol,
    /// Key selection; absent = keep (new endpoints: `RoundRobin`).
    #[serde(default)]
    #[ts(optional)]
    pub key_strategy: Option<KeyStrategy>,
    /// Selected models in display order.
    pub models: Vec<ModelSave>,
    /// Image generation base URL: empty/null = use `baseUrl`; absent = keep.
    #[serde(default, with = "present")]
    #[ts(type = "string | null", optional)]
    pub image_base_url: Option<Option<String>>,
    /// Image edit (`/images/edits`) base URL: empty/null = use `base_url`; absent = keep.
    #[serde(default, with = "present")]
    #[ts(type = "string | null", optional)]
    pub image_edit_base_url: Option<Option<String>>,
    /// Custom request headers; absent = keep.
    #[serde(default)]
    #[ts(optional)]
    pub headers: Option<BTreeMap<String, String>>,
}

mod present {
    use serde::Deserialize;

    pub(super) fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::deserialize(deserializer).map(Some)
    }
}

/// Adds (no `id`) or patches (with `id`) one key of an endpoint; omitted fields keep.
#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct EndpointKeySave {
    /// Endpoint the key belongs to.
    pub endpoint_id: String,
    /// Existing key id; absent = new key.
    #[serde(default)]
    #[ts(optional)]
    pub id: Option<String>,
    /// Plaintext key, at most 4096 bytes; required for a new key.
    #[serde(default)]
    #[ts(optional)]
    pub key: Option<String>,
    /// Label, at most 40 characters; new keys default to `""`.
    #[serde(default)]
    #[ts(optional)]
    pub label: Option<String>,
    /// New keys default to enabled.
    #[serde(default)]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// `true` clears a set-aside state so the key is tried again.
    #[serde(default)]
    #[ts(optional)]
    pub restore: Option<bool>,
}

/// A whole-provider save: endpoints missing from `endpoints` are deleted.
#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ProviderSave {
    /// Existing provider id; absent = new provider.
    #[serde(default)]
    #[ts(optional)]
    pub id: Option<String>,
    /// Display name, 1–80 characters.
    pub name: String,
    /// Absent = keep (new providers start enabled).
    #[serde(default)]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// Markdown notes, at most 10000 characters; absent = keep.
    #[serde(default)]
    #[ts(optional)]
    pub notes: Option<String>,
    /// New providers only: seed this host preset. A no-op returning the current list when the preset
    /// is already present; a same-named user provider adopts it instead of a duplicate.
    #[serde(default)]
    #[ts(optional)]
    pub preset_id: Option<String>,
    /// The complete endpoint list.
    pub endpoints: Vec<EndpointSave>,
}

/// Reference to one stored model, kept by the host for its own role settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ModelRef {
    /// Endpoint id.
    pub endpoint_id: String,
    /// Model id within the endpoint.
    pub model_id: String,
}

/// One entry of a model picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelOption {
    /// Endpoint id.
    pub endpoint_id: String,
    /// Model id.
    pub model_id: String,
    /// `provider · endpoint · model`.
    pub label: String,
    /// User display name of the model; `None` = not set.
    pub name: Option<String>,
    /// Model type.
    pub model_type: ModelType,
    /// Whether the model accepts tools; `None` = unknown.
    pub supports_tools: Option<bool>,
    /// Configured reasoning capabilities.
    pub reasoning: Option<aether_support::ReasoningConfig>,
    /// User-declared support for protocol-native web search.
    pub native_web_search: bool,
    /// Image input support (inferred on fetch, user-editable in model settings).
    pub can_vision: bool,
}

/// Narrows [`crate::Store::model_options`]; `None` fields match everything.
#[derive(Debug, Clone, Copy, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ModelFilter {
    /// Only this type.
    #[ts(optional)]
    pub model_type: Option<ModelType>,
    /// Only models whose `supports_tools` equals this value.
    #[ts(optional)]
    pub supports_tools: Option<bool>,
}

impl ModelType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Image => "image",
            Self::Video => "video",
            Self::Tts => "tts",
            Self::Stt => "stt",
            Self::Embedding => "embedding",
            Self::Rerank => "rerank",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "chat" => Self::Chat,
            "image" => Self::Image,
            "video" => Self::Video,
            "tts" => Self::Tts,
            "stt" => Self::Stt,
            "embedding" => Self::Embedding,
            "rerank" => Self::Rerank,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

impl TypeSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Inferred => "inferred",
            Self::User => "user",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "inferred" => Some(Self::Inferred),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}
/// What to probe: a stored endpoint, or an unsaved draft from the settings form (docs/11 §4).
#[derive(Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ProbeTarget {
    /// A stored endpoint with its stored keys.
    Saved {
        /// Endpoint id.
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    /// Unsaved form values; keys come from `endpointId` when given.
    #[serde(rename_all = "camelCase")]
    Draft {
        /// Base URL as typed.
        base_url: String,
        /// Wire protocol.
        protocol: Protocol,
        /// Endpoint being edited, if any.
        #[serde(default)]
        #[ts(optional)]
        endpoint_id: Option<String>,
        /// Use only this key of `endpointId`; absent = rotate with failover.
        #[serde(default)]
        #[ts(optional)]
        key_id: Option<String>,
    },
}

/// Why fetching the model list failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProbeErrorKind {
    /// No key and the address is not loopback; no request was sent.
    MissingKey,
    /// 401 / 403.
    Unauthorized,
    /// 404.
    NotFound,
    /// 429.
    RateLimited,
    /// Any other non-2xx status.
    Status,
    /// Deadline elapsed.
    Timeout,
    /// DNS, connect or TLS failure.
    Connect,
    /// Rejected by the address policy.
    BlockedAddress,
    /// Unparseable or empty model list.
    InvalidBody,
}

/// A probe failure shown next to the endpoint form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbeError {
    /// Failure kind.
    pub kind: ProbeErrorKind,
    /// Upstream HTTP status.
    pub status: Option<u16>,
    /// Upstream text, masked, at most 300 characters.
    pub message: Option<String>,
    /// i18n key such as `providers.hint.missing_v1`.
    pub hint: Option<String>,
}

/// A model found by a probe, with the inferred family group.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbedModel {
    /// Inferred model metadata.
    #[serde(flatten)]
    #[ts(flatten)]
    pub model: Model,
    /// Family group for the picker, when known.
    pub group: Option<String>,
}

/// Probe outcome: never a command error, so the UI can show `probeUrl` either way.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbeResult {
    /// `GET` URL that was (or would be) requested.
    pub probe_url: String,
    /// Deduplicated, sorted models; empty on error.
    pub models: Vec<ProbedModel>,
    /// Set on failure.
    pub error: Option<ProbeError>,
}
