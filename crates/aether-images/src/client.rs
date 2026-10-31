//! Native OpenAI-compatible image transport.

use crate::{ImagePart, ImageRequest, MAX_EDIT_IMAGES, MAX_IMAGE_BYTES, decode};
use aether_support::{
    Connection, KitError, KitErrorCode,
    net::{self, AddressPolicy},
};
use base64::Engine as _;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(120);
const RESPONSE_BYTES: usize = MAX_IMAGE_BYTES.div_ceil(3) * 4 + 64 * 1024;

/// Generates or edits one image using an explicit request and cooperative cancellation.
///
/// # Errors
/// Invalid prompt/reference input, unsupported editing, address-policy rejection,
/// upstream failure, cancellation or a 120-second total timeout.
pub async fn generate(
    connection: &Connection,
    policy: &AddressPolicy,
    request: &ImageRequest,
    cancel: &CancellationToken,
) -> Result<ImagePart, KitError> {
    let prompt = request.prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > 4000 {
        return Err(KitError::bad_request("prompt", "invalid"));
    }
    if request.images.len() > MAX_EDIT_IMAGES {
        return Err(KitError::bad_request("images", "count").with_limit(MAX_EDIT_IMAGES as u64));
    }
    if !request.images.is_empty() && !connection.can_vision {
        return Err(KitError::new(
            KitErrorCode::Unsupported,
            "image editing unsupported",
        ));
    }
    if cancel.is_cancelled() {
        return Err(KitError::cancelled());
    }
    tokio::select! {
        () = cancel.cancelled() => Err(KitError::cancelled()),
        result = tokio::time::timeout(TIMEOUT, send(connection, policy, request, prompt)) => {
            result.map_err(|_| KitError::new(KitErrorCode::Timeout, "image request timeout"))?
        }
    }
}

async fn send(
    connection: &Connection,
    policy: &AddressPolicy,
    input: &ImageRequest,
    prompt: &str,
) -> Result<ImagePart, KitError> {
    let (base, path) = if input.images.is_empty() {
        (connection.image_base_url.as_deref(), "generations")
    } else {
        (connection.image_edit_base_url.as_deref(), "edits")
    };
    let url = net::check_url(
        &format!(
            "{}/images/{path}",
            base.unwrap_or(&connection.base_url).trim_end_matches('/')
        ),
        policy,
    )?;
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let request = net::custom_headers(client.post(url), &connection.headers);
    // Serialize once; rotating credentials must not rebuild or copy the full body.
    let request = if input.images.is_empty() {
        request.json(&json!({
            "model": connection.model_id,
            "prompt": prompt,
            "size": input.size.as_str(),
            "n": 1,
        }))
    } else {
        let boundary = boundary()?;
        let body = multipart(
            &boundary,
            &[
                ("model", connection.model_id.as_deref().unwrap_or_default()),
                ("prompt", prompt),
                ("size", input.size.as_str()),
                ("n", "1"),
            ],
            &input.images,
        )?;
        request
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
    };
    let known: Vec<&str> = connection.api_keys.iter().map(|key| key.as_str()).collect();
    let attempts = known.len().max(1);
    let mut attempt = 0;
    let (status, body) = loop {
        let mut next = request
            .try_clone()
            .ok_or_else(|| KitError::internal(std::io::Error::other("unclonable image request")))?;
        if let Some(key) = known.get(attempt) {
            next = next.bearer_auth(key);
        }
        let response = next.send().await.map_err(net::transport)?;
        let status = response.status();
        let bytes = bounded(
            response,
            if status.is_success() {
                RESPONSE_BYTES
            } else {
                8192
            },
        )
        .await?;
        if status.is_success() {
            let body: Value = serde_json::from_slice(&bytes).map_err(|error| {
                KitError::upstream(status.as_u16(), "invalid image response", &known)
                    .with_cause(error)
            })?;
            break (status, body);
        }
        let error = KitError::upstream(status.as_u16(), &String::from_utf8_lossy(&bytes), &known);
        if attempt + 1 < attempts && net::next_key(&error) {
            attempt += 1;
        } else {
            return Err(error);
        }
    };
    let first = &body["data"][0];
    let bytes = if let Some(data) = first["b64_json"].as_str() {
        // Bound encoded length before decoding untrusted provider data.
        if data.len() / 4 * 3 > MAX_IMAGE_BYTES + 2 {
            return Err(KitError::bad_request("images", "size"));
        }
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| KitError::bad_request("images", "encoding"))?
    } else if let Some(link) = first["url"].as_str() {
        let url = net::check_url(link, policy)?;
        let response = client.get(url).send().await.map_err(net::transport)?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let bytes = bounded(response, 8192).await?;
            return Err(KitError::upstream(
                status,
                &String::from_utf8_lossy(&bytes),
                &known,
            ));
        }
        bounded(response, MAX_IMAGE_BYTES).await?
    } else {
        return Err(KitError::upstream(
            status.as_u16(),
            "no image in response",
            &known,
        ));
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(KitError::bad_request("images", "size"));
    }
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        return Err(KitError::bad_request("images", "mime"));
    };
    Ok(ImagePart {
        mime: mime.to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, KitError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(net::transport)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(KitError::bad_request("images", "size").with_limit(limit as u64));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn boundary() -> Result<String, KitError> {
    use std::fmt::Write as _;
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random)
        .map_err(|e| KitError::internal(std::io::Error::other(e.to_string())))?;
    let mut boundary = String::from("aether-");
    for byte in random {
        write!(boundary, "{byte:02x}").map_err(KitError::internal)?;
    }
    Ok(boundary)
}

fn multipart(
    boundary: &str,
    fields: &[(&str, &str)],
    images: &[ImagePart],
) -> Result<Vec<u8>, KitError> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    let field = if images.len() == 1 {
        "image"
    } else {
        "image[]"
    };
    for (index, image) in images.iter().enumerate() {
        let bytes = decode(image)?;
        let ext = match image.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\";\
 filename=\"image-{}.{ext}\"\r\nContent-Type: {}\r\n\r\n",
                index + 1,
                image.mime
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(body)
}
