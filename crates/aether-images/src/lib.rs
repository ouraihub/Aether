//! Independent image contracts, validation and optional Native generation/editing.

use aether_support::KitError;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::generate;

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
pub struct ImageRequest {
    /// Nonempty prompt, at most 4000 Unicode characters after trimming.
    pub prompt: String,
    /// One supported output size.
    pub size: ImageSize,
    /// Empty for generation, otherwise reference images for editing.
    pub images: Vec<ImagePart>,
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
    if !IMAGE_MIMES.contains(&image.mime.as_str()) {
        return Err(KitError::bad_request("images", "mime"));
    }
    let too_big = || KitError::bad_request("images", "size").with_limit(MAX_IMAGE_BYTES as u64);
    if image.data.len() / 4 * 3 > MAX_IMAGE_BYTES + 2 {
        return Err(too_big());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.data)
        .map_err(|_| KitError::bad_request("images", "encoding"))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(too_big());
    }
    Ok(bytes)
}
