use crate::{ImageGeneration, ImagePart, ImageProfile, ImageRequest, decode_base64, validate};
use aether_support::{
    Connection, KitError, TokenUsage,
    net::{self, AddressPolicy},
};
use base64::Engine as _;
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};

pub(super) async fn send(
    connection: &Connection,
    policy: &AddressPolicy,
    input: &ImageRequest,
    limits: &crate::ImageLimits,
    prompt: &str,
    prepared: (ImageProfile, &[Vec<u8>]),
    dispatched: &AtomicU8,
) -> Result<ImageGeneration, KitError> {
    let (profile, decoded) = prepared;
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
    if url.query().is_some() || url.fragment().is_some() {
        return Err(KitError::bad_request("url", "invalid"));
    }
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let request = net::custom_headers(client.post(url), &connection.headers);
    let request = if input.images.is_empty() {
        let mut body = json!({"model": connection.model_id, "prompt": prompt});
        if profile != ImageProfile::SiliconFlow {
            body["n"] = json!(1);
        }
        body[if profile == ImageProfile::SiliconFlow {
            "image_size"
        } else {
            "size"
        }] = json!(input.size.as_str());
        if profile == ImageProfile::GptImage {
            body["output_format"] = json!("png");
            body["background"] = json!("opaque");
            if let Some(quality) = input.quality {
                body["quality"] = json!(quality.as_str());
            }
        }
        request.json(&body)
    } else {
        let boundary = boundary()?;
        let mut fields = vec![
            ("model", connection.model_id.as_deref().unwrap_or_default()),
            ("prompt", prompt),
            ("size", input.size.as_str()),
            ("n", "1"),
        ];
        if profile == ImageProfile::GptImage {
            fields.extend([("output_format", "png"), ("background", "opaque")]);
            if let Some(quality) = input.quality {
                fields.push(("quality", quality.as_str()));
            }
        }
        request
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(multipart(
                &boundary,
                &fields,
                &input.images,
                input.mask.as_ref(),
                decoded,
            ))
    };
    let known: Vec<&str> = connection.api_keys.iter().map(|key| key.as_str()).collect();
    let attempts = known.len().max(1);
    let mut attempt = 0;
    let body = loop {
        let mut next = request
            .try_clone()
            .ok_or_else(|| KitError::internal(std::io::Error::other("unclonable image request")))?;
        if let Some(key) = known.get(attempt) {
            next = next.bearer_auth(key);
        }
        let next = next.build().map_err(net::transport)?;
        dispatched.store(1, Ordering::Relaxed);
        let response = client.execute(next).await.map_err(net::transport)?;
        let status = response.status();
        if status.is_client_error() {
            dispatched.store(2, Ordering::Relaxed);
        }
        let bytes = bounded(
            response,
            if status.is_success() {
                limits.max_image_bytes.div_ceil(3) * 4 + 64 * 1024
            } else {
                8192
            },
        )
        .await?;
        if status.is_success() {
            break serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                KitError::upstream(status.as_u16(), "invalid image response", &known)
                    .with_cause(error)
            })?;
        }
        let error = KitError::upstream(status.as_u16(), &String::from_utf8_lossy(&bytes), &known);
        if attempt + 1 < attempts && net::next_key(&error) {
            attempt += 1;
        } else {
            return Err(error);
        }
    };
    let invalid_response = || {
        KitError::new(
            aether_support::KitErrorCode::Upstream,
            "invalid image response",
        )
    };
    let images = body[if profile == ImageProfile::SiliconFlow {
        "images"
    } else {
        "data"
    }]
    .as_array()
    .filter(|images| images.len() == 1)
    .ok_or_else(invalid_response)?;
    let first = &images[0];
    if first["b64_json"].is_string() == first["url"].is_string() {
        return Err(invalid_response());
    }
    let (bytes, reported_mime) = if let Some(data) = first["b64_json"].as_str() {
        (decode_base64(data, limits.max_image_bytes)?, None)
    } else if let Some(link) = first["url"].as_str() {
        let url = net::check_url(link, policy)?;
        if url.fragment().is_some() {
            return Err(KitError::bad_request("url", "invalid"));
        }
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
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|value| {
                value
                    .to_str()
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            });
        (bounded(response, limits.max_image_bytes).await?, mime)
    } else {
        return Err(invalid_response());
    };
    if bytes.len() > limits.max_image_bytes {
        return Err(
            KitError::bad_request("images", "size").with_limit(limits.max_image_bytes as u64)
        );
    }
    let mime = validate::sniff(&bytes)?;
    if reported_mime.as_deref().is_some_and(|reported| {
        !reported.is_empty() && reported != "application/octet-stream" && reported != mime
    }) {
        return Err(KitError::bad_request("images", "mime"));
    }
    validate::validate(&bytes, mime)?;
    Ok(ImageGeneration {
        image: ImagePart {
            mime: mime.to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        },
        revised_prompt: first["revised_prompt"].as_str().map(str::to_owned),
        usage: usage(&body["usage"]),
        total_tokens: body["usage"]["total_tokens"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok()),
    })
}

fn usage(value: &Value) -> TokenUsage {
    let count = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
    TokenUsage {
        input_tokens: count(&value["input_tokens"]).or_else(|| count(&value["prompt_tokens"])),
        output_tokens: count(&value["output_tokens"])
            .or_else(|| count(&value["completion_tokens"])),
        cache_read_tokens: count(&value["input_tokens_details"]["cached_tokens"])
            .or_else(|| count(&value["prompt_tokens_details"]["cached_tokens"])),
        cache_write_tokens: count(&value["input_tokens_details"]["cache_creation_tokens"])
            .or_else(|| count(&value["cache_creation_input_tokens"])),
        reasoning_tokens: count(&value["output_tokens_details"]["reasoning_tokens"])
            .or_else(|| count(&value["completion_tokens_details"]["reasoning_tokens"])),
    }
}

async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, KitError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(KitError::bad_request("images", "size").with_limit(limit as u64));
    }
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
    mask: Option<&ImagePart>,
    decoded: &[Vec<u8>],
) -> Vec<u8> {
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
    for ((index, image), bytes) in images
        .iter()
        .enumerate()
        .chain(mask.into_iter().map(|image| (images.len(), image)))
        .zip(decoded)
    {
        let ext = match image.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        let name = if index == images.len() { "mask" } else { field };
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"image-{}.{ext}\"\r\nContent-Type: {}\r\n\r\n",
                index + 1,
                image.mime
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}
