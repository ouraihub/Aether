//! Independent image contracts, validation and optional Native generation/editing.

use aether_support::KitError;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::{ImageCapabilities, generate, image_capabilities};

#[cfg(feature = "client")]
mod validate;

/// Decoded bytes permitted per image.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// Supported image MIME types.
pub const IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];
/// Reference images permitted per independent edit request.
pub const MAX_EDIT_IMAGES: usize = 4;

/// Standard base64 image bytes without a data URL prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ImagePart {
    /// Supported MIME type.
    pub mime: String,
    /// Standard base64 bytes.
    pub data: String,
}

/// Host-controlled display reference; expired images retain their MIME.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageRef {
    /// Supported MIME type.
    pub mime: String,
    /// Controlled display URL, absent after expiry.
    pub url: Option<String>,
}

/// Explicit independent generation/editing request, unrelated to chat runs.
#[derive(Debug, Default)]
pub struct ImageRequest {
    /// Nonempty prompt, at most 4000 Unicode characters after trimming.
    pub prompt: String,
    /// One supported output size.
    pub size: ImageSize,
    /// Empty for generation, otherwise reference images for editing.
    pub images: Vec<ImagePart>,
    /// GPT image quality; unsupported profiles reject it explicitly.
    pub quality: Option<ImageQuality>,
    /// PNG edit mask, requiring at least one reference image.
    pub mask: Option<ImagePart>,
}

/// Explicit image protocol profile; independent of chat vision capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProfile {
    /// Portable OpenAI-compatible generation and editing.
    Generic,
    /// GPT Image quality, opaque PNG output and masked editing.
    GptImage,
    /// SiliconFlow image_size requests and images responses.
    SiliconFlow,
}

/// Supported GPT image quality values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageQuality {
    /// Low quality.
    Low,
    /// Medium quality.
    Medium,
    /// High quality.
    High,
}

impl ImageQuality {
    /// Provider wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Independent image execution budget; does not enlarge chat attachment limits.
#[derive(Debug, Clone, Copy)]
pub struct ImageLimits {
    /// Maximum decoded independent input/output bytes (at most 10 MiB).
    pub max_image_bytes: usize,
    /// Total request and download budget (at most 180,000 ms).
    pub timeout_ms: u64,
    /// Explicit profile override, otherwise known model IDs/endpoint hosts are used.
    pub profile: Option<ImageProfile>,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_image_bytes: MAX_IMAGE_BYTES,
            timeout_ms: 120_000,
            profile: None,
        }
    }
}

/// Validated independent generation result, without product storage identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGeneration {
    /// Fully validated image bytes encoded as standard base64.
    pub image: ImagePart,
    /// Provider-reported revised prompt, if present.
    pub revised_prompt: Option<String>,
    /// Only real reported counts; unknown counts remain null.
    pub usage: aether_support::TokenUsage,
    /// Provider-reported total, independent of unavailable input/output counts.
    pub total_tokens: Option<u32>,
}

/// Dispatch fact retained independently from the underlying error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStage {
    /// No generation request was sent.
    NotDispatched,
    /// An explicit provider response rejected the request.
    Rejected,
    /// Generation was dispatched and its result cannot be confirmed; never replay.
    Unknown,
    /// Generation succeeded, but subsequent product storage failed.
    Generated,
}

/// An image failure retaining the dispatch stage that a KitError alone cannot carry.
#[derive(Debug)]
pub struct ImageFailure {
    /// Dispatch state; callers must inspect before considering another generation.
    pub stage: ImageStage,
    /// Original structured error.
    pub error: KitError,
}

impl ImageFailure {
    /// Explicitly crosses a KitError-only boundary without losing dispatch facts.
    #[must_use]
    pub fn into_kit_error(self) -> KitError {
        match self.stage {
            ImageStage::NotDispatched | ImageStage::Rejected => self.error,
            ImageStage::Unknown | ImageStage::Generated => KitError::new(
                self.error.code(),
                format!(
                    "Image {:?}: {}. Do not automatically generate again.",
                    self.stage,
                    self.error.message()
                ),
            )
            .with_cause(self.error),
        }
    }
}

impl std::fmt::Display for ImageFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "image {:?}: {}", self.stage, self.error)
    }
}

impl std::error::Error for ImageFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Supported image output dimensions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageSize {
    /// 1024 by 1024 pixels.
    #[default]
    Square,
    /// 1024 by 1536 pixels.
    Portrait,
    /// 1536 by 1024 pixels.
    Landscape,
}

impl ImageSize {
    /// OpenAI-compatible size value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Square => "1024x1024",
            Self::Portrait => "1024x1536",
            Self::Landscape => "1536x1024",
        }
    }
}

/// Checks MIME, standard base64 encoding and decoded byte limits without a count policy.
///
/// # Errors
/// `bad_request{images,mime|encoding|size}` for an invalid image.
pub fn check_images(images: &[ImagePart]) -> Result<(), KitError> {
    for image in images {
        decode(image)?;
    }
    Ok(())
}

fn decode(image: &ImagePart) -> Result<Vec<u8>, KitError> {
    decode_with_limit(image, MAX_IMAGE_BYTES)
}

fn decode_with_limit(image: &ImagePart, limit: usize) -> Result<Vec<u8>, KitError> {
    if !IMAGE_MIMES.contains(&image.mime.as_str()) {
        return Err(KitError::bad_request("images", "mime"));
    }
    decode_base64(&image.data, limit)
}

fn decode_base64(data: &str, limit: usize) -> Result<Vec<u8>, KitError> {
    let too_big = || KitError::bad_request("images", "size").with_limit(limit as u64);
    if data.len() > limit.div_ceil(3) * 4 {
        return Err(too_big());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| KitError::bad_request("images", "encoding"))?;
    if bytes.len() > limit {
        return Err(too_big());
    }
    Ok(bytes)
}
