//! Native image generation/editing with explicit dispatch outcomes.

use crate::{
    ImageFailure, ImageGeneration, ImageLimits, ImagePart, ImageProfile, ImageRequest, ImageStage,
    MAX_EDIT_IMAGES, decode_base64, decode_with_limit, validate,
};
use aether_support::{
    Connection, KitError, KitErrorCode, TokenUsage,
    net::{self, AddressPolicy},
};
use base64::Engine as _;
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// Explicit capabilities of the selected image protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageCapabilities {
    /// Selected protocol profile.
    pub profile: ImageProfile,
    /// Reference image editing is supported.
    pub edit: bool,
    /// Explicit quality is supported.
    pub quality: bool,
    /// PNG mask editing is supported, independently of chat vision.
    pub mask: bool,
}

/// Resolves capabilities without making a network request.
#[must_use]
pub fn image_capabilities(connection: &Connection, limits: &ImageLimits) -> ImageCapabilities {
    let profile = limits.profile.unwrap_or_else(|| {
        let base = connection
            .image_base_url
            .as_deref()
            .unwrap_or(&connection.base_url);
        let silicon = net::check_url(base, &AddressPolicy::default())
            .ok()
            .is_some_and(|url| {
                matches!(
                    url.host_str(),
                    Some("api.siliconflow.cn" | "api.siliconflow.com")
                )
            });
        if silicon {
            ImageProfile::SiliconFlow
        } else if matches!(
            connection.model_id.as_deref(),
            Some("gpt-image-1" | "gpt-image-1-mini" | "gpt-image-1.5" | "gpt-image-2")
        ) {
            ImageProfile::GptImage
        } else {
            ImageProfile::Generic
        }
    });
    ImageCapabilities {
        profile,
        edit: connection.can_vision && profile != ImageProfile::SiliconFlow,
        quality: profile == ImageProfile::GptImage,
        mask: profile == ImageProfile::GptImage,
    }
}

/// Generates or edits one validated image; never retries an uncertain dispatch.
///
/// # Errors
/// Input/profile/policy failures retain `NotDispatched`, explicit 4xx rejection
/// retains `Rejected`, and uncertain dispatch or invalid output retains `Unknown`.
/// Limits include generation, key rotation, response reads and downloads.
pub async fn generate(
    connection: &Connection,
    policy: &AddressPolicy,
    request: &ImageRequest,
    limits: &ImageLimits,
    cancel: &CancellationToken,
) -> Result<ImageGeneration, ImageFailure> {
    let dispatched = AtomicU8::new(0);
    let outcome = async {
        let prompt = request.prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > 4000 {
            return Err(KitError::bad_request("prompt", "invalid"));
        }
        if limits.max_image_bytes == 0
            || limits.max_image_bytes > 10 * 1024 * 1024
            || limits.timeout_ms == 0
            || limits.timeout_ms > 180_000
        {
            return Err(KitError::bad_request("limits", "invalid"));
        }
        if request.images.len() > MAX_EDIT_IMAGES {
            return Err(KitError::bad_request("images", "count").with_limit(MAX_EDIT_IMAGES as u64));
        }
        let capabilities = image_capabilities(connection, limits);
        if !request.images.is_empty() && !capabilities.edit {
            return Err(KitError::unsupported("image editing"));
        }
        if request.quality.is_some() && !capabilities.quality {
            return Err(KitError::unsupported("image quality"));
        }
        if request.mask.is_some() && (!capabilities.mask || request.images.is_empty()) {
            return Err(KitError::unsupported("image mask"));
        }
        let mut total = 0;
        let mut first_dimensions = None;
        let mut decoded =
            Vec::with_capacity(request.images.len() + usize::from(request.mask.is_some()));
        for image in &request.images {
            let bytes = decode_with_limit(image, limits.max_image_bytes)?;
            total += bytes.len();
            if total > 20 * 1024 * 1024 {
                return Err(KitError::bad_request("images", "size").with_limit(20 * 1024 * 1024));
            }
            let dimensions = validate::validate(&bytes, &image.mime)?;
            if first_dimensions.is_none() {
                first_dimensions = Some(dimensions);
            }
            decoded.push(bytes);
        }
        if let Some(mask) = &request.mask {
            if mask.mime != "image/png" {
                return Err(KitError::bad_request("mask", "invalid"));
            }
            let first =
                first_dimensions.ok_or_else(|| KitError::bad_request("mask", "requires_image"))?;
            let bytes = decode_with_limit(mask, limits.max_image_bytes)?;
            total += bytes.len();
            if total > 20 * 1024 * 1024 {
                return Err(KitError::bad_request("images", "size").with_limit(20 * 1024 * 1024));
            }
            let dimensions = validate::validate(&bytes, &mask.mime)?;
            if !dimensions.alpha
                || dimensions.width != first.width
                || dimensions.height != first.height
            {
                return Err(KitError::bad_request("mask", "invalid"));
            }
            decoded.push(bytes);
        }
        if cancel.is_cancelled() {
            return Err(KitError::cancelled());
        }
        send(
            connection,
            policy,
            request,
            limits,
            prompt,
            (capabilities.profile, &decoded),
            &dispatched,
        )
        .await
    };
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(KitError::cancelled()),
        result = tokio::time::timeout(Duration::from_millis(limits.timeout_ms), outcome) => result.unwrap_or_else(|_| Err(KitError::new(KitErrorCode::Timeout, "image request timeout"))),
    };
    result.map_err(|error| ImageFailure {
        stage: match dispatched.load(Ordering::Relaxed) {
            0 => ImageStage::NotDispatched,
            2 => ImageStage::Rejected,
            _ => ImageStage::Unknown,
        },
        error,
    })
}

async fn send(
    connection: &Connection,
    policy: &AddressPolicy,
    input: &ImageRequest,
    limits: &ImageLimits,
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
        // Build before marking dispatch: malformed headers never left this process.
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
    let invalid_response = || KitError::new(KitErrorCode::Upstream, "invalid image response");
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
        // Separate request: provider headers and bearer credentials are never copied.
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
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"image-{}.{ext}\"\r\nContent-Type: {}\r\n\r\n", index + 1, image.mime).as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "a failed assertion is a failed test")]
mod tests {
    use super::*;
    use aether_support::Protocol;
    use aether_testkit::{Exhausted, Reply, Server};
    use zeroize::Zeroizing;

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==";
    fn connection(base: &str) -> Connection {
        Connection {
            base_url: base.to_owned(),
            protocol: Protocol::OpenaiChat,
            api_keys: vec![
                Zeroizing::new("sk-paint".into()),
                Zeroizing::new("sk-paint-2".into()),
            ],
            model_id: Some("gpt-image-2".into()),
            endpoint_id: None,
            context_window: None,
            max_output: None,
            supports_tools: None,
            reasoning: None,
            native_web_search: false,
            pricing: None,
            can_vision: true,
            headers: vec![("X-Relay-Tag".into(), "secret".into())],
            image_base_url: None,
            image_edit_base_url: None,
        }
    }
    fn input() -> ImageRequest {
        ImageRequest {
            prompt: " fox ".into(),
            ..ImageRequest::default()
        }
    }
    fn success() -> Reply {
        Reply::json(format!(
            r#"{{"data":[{{"b64_json":"{PNG}","revised_prompt":"red fox"}}],"usage":{{"input_tokens":9,"output_tokens":2,"total_tokens":14}}}}"#
        ))
    }
    async fn run(
        connection: &Connection,
        input: &ImageRequest,
        limits: &ImageLimits,
    ) -> Result<ImageGeneration, ImageFailure> {
        generate(
            connection,
            &AddressPolicy::default(),
            input,
            limits,
            &CancellationToken::new(),
        )
        .await
    }

    #[tokio::test]
    async fn profiles_limits_outputs_and_dispatch_facts() {
        let server = Server::script(
            vec![Reply::status(429, "{}"), success()],
            Exhausted::Reply(Reply::close()),
        )
        .await;
        let mut request = input();
        request.quality = Some(crate::ImageQuality::High);
        let generated = run(
            &connection(&server.url()),
            &request,
            &ImageLimits::default(),
        )
        .await
        .unwrap();
        assert_eq!(generated.image.data, PNG);
        assert_eq!(generated.revised_prompt.as_deref(), Some("red fox"));
        assert_eq!(generated.usage.input_tokens, Some(9));
        assert_eq!(generated.total_tokens, Some(14));
        assert_eq!(server.seen().len(), 2);
        let body: Value = serde_json::from_slice(&server.seen()[1].body).unwrap();
        assert_eq!(
            body,
            json!({"model":"gpt-image-2","prompt":"fox","size":"1024x1024","n":1,"quality":"high","output_format":"png","background":"opaque"})
        );
        for (reply, stage) in [
            (Reply::status(400, "sk-paint"), ImageStage::Rejected),
            (Reply::status(500, "{}"), ImageStage::Unknown),
            (Reply::json("{}"), ImageStage::Unknown),
            (Reply::close(), ImageStage::Unknown),
        ] {
            let server = Server::script(vec![reply], Exhausted::Reply(Reply::close())).await;
            let failure = run(
                &connection(&server.url()),
                &input(),
                &ImageLimits::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(failure.stage, stage);
            assert!(!failure.error.message().contains("sk-paint"));
            assert_eq!(server.seen().len(), 1);
        }
        let server = Server::script(vec![success()], Exhausted::Reply(Reply::close())).await;
        let generic = ImageLimits {
            profile: Some(ImageProfile::Generic),
            ..ImageLimits::default()
        };
        assert_eq!(
            run(&connection(&server.url()), &request, &generic)
                .await
                .unwrap_err()
                .stage,
            ImageStage::NotDispatched
        );
        assert!(server.seen().is_empty());
        let too_small = ImageLimits {
            max_image_bytes: 1,
            ..ImageLimits::default()
        };
        assert_eq!(
            run(&connection(&server.url()), &input(), &too_small)
                .await
                .unwrap_err()
                .stage,
            ImageStage::Unknown
        );
    }

    #[tokio::test]
    async fn url_downloads_sniff_without_secrets_and_check_declared_mime() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(PNG)
            .unwrap();
        for (mime, valid) in [
            (None, true),
            (Some("application/octet-stream"), true),
            (Some("image/png"), true),
            (Some("image/jpeg"), false),
            (Some("text/html"), false),
        ] {
            let headers = mime
                .map(|mime| vec![("content-type", mime)])
                .unwrap_or_default();
            let files = Server::script(
                vec![Reply::http(200, &headers, bytes.clone())],
                Exhausted::Reply(Reply::close()),
            )
            .await;
            let server = Server::script(
                vec![Reply::json(format!(
                    r#"{{"data":[{{"url":"{}/image"}}]}}"#,
                    files.url()
                ))],
                Exhausted::Reply(Reply::close()),
            )
            .await;
            let result = run(
                &connection(&server.url()),
                &input(),
                &ImageLimits::default(),
            )
            .await;
            assert_eq!(result.is_ok(), valid);
            assert!(files.seen()[0].header("authorization").is_none());
            assert!(files.seen()[0].header("x-relay-tag").is_none());
        }
    }

    #[tokio::test]
    async fn mask_multipart_and_siliconflow_and_validation() {
        let server = Server::script(vec![success()], Exhausted::Reply(Reply::close())).await;
        let image = ImagePart {
            mime: "image/png".into(),
            data: PNG.into(),
        };
        let mut request = input();
        request.images = vec![image.clone(), image.clone()];
        request.mask = Some(image);
        request.quality = Some(crate::ImageQuality::Low);
        run(
            &connection(&server.url()),
            &request,
            &ImageLimits::default(),
        )
        .await
        .unwrap();
        let seen = server.seen();
        assert!(seen[0].target.ends_with("/images/edits"));
        let body = seen[0].text();
        assert_eq!(body.matches("name=\"image[]\"").count(), 2);
        assert!(body.contains("name=\"mask\""));
        assert!(body.contains("name=\"quality\"\r\n\r\nlow"));
        drop(seen);
        let sf = ImageLimits {
            profile: Some(ImageProfile::SiliconFlow),
            ..ImageLimits::default()
        };
        assert_eq!(
            run(&connection(&server.url()), &request, &sf)
                .await
                .unwrap_err()
                .stage,
            ImageStage::NotDispatched
        );
        let server = Server::script(
            vec![Reply::json(format!(
                r#"{{"images":[{{"b64_json":"{PNG}"}}]}}"#
            ))],
            Exhausted::Reply(Reply::close()),
        )
        .await;
        run(&connection(&server.url()), &input(), &sf)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&server.seen()[0].body).unwrap();
        assert_eq!(body["image_size"], "1024x1024");
        assert!(body.get("size").is_none() && body.get("response_format").is_none());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(PNG)
            .unwrap();
        assert!(validate::validate(&bytes, "image/png").is_ok());
        assert!(validate::validate(&bytes[..24], "image/png").is_err());
        assert!(validate::validate(&bytes, "image/jpeg").is_err());
        let mut huge = bytes;
        huge[16..20].copy_from_slice(&40_000_001_u32.to_be_bytes());
        assert!(validate::validate(&huge, "image/png").is_err());
        let gif = base64::engine::general_purpose::STANDARD
            .decode("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
            .unwrap();
        assert!(validate::validate(&gif, "image/gif").is_ok());
        assert!(validate::validate(&gif[..gif.len() - 1], "image/gif").is_err());
    }

    #[tokio::test]
    async fn cancellation_deadline_and_large_independent_inputs() {
        let server = Server::script(vec![success()], Exhausted::Reply(Reply::close())).await;
        let token = CancellationToken::new();
        token.cancel();
        let failure = generate(
            &connection(&server.url()),
            &AddressPolicy::default(),
            &input(),
            &ImageLimits::default(),
            &token,
        )
        .await
        .unwrap_err();
        assert_eq!(failure.stage, ImageStage::NotDispatched);
        assert!(server.seen().is_empty());
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let server = Server::script(
            vec![Reply::default().then_signal(&notify).then_stall()],
            Exhausted::Reply(Reply::close()),
        )
        .await;
        let token = CancellationToken::new();
        let cancel = async {
            notify.notified().await;
            token.cancel();
        };
        let conn = connection(&server.url());
        let request = input();
        let policy = AddressPolicy::default();
        let limits = ImageLimits::default();
        let (result, ()) =
            tokio::join!(generate(&conn, &policy, &request, &limits, &token), cancel);
        assert_eq!(result.unwrap_err().stage, ImageStage::Unknown);
        assert_eq!(server.seen().len(), 1);
        let server = Server::script(vec![Reply::silent()], Exhausted::Reply(Reply::close())).await;
        let deadline = ImageLimits {
            timeout_ms: 50,
            ..ImageLimits::default()
        };
        let failure = run(&connection(&server.url()), &input(), &deadline)
            .await
            .unwrap_err();
        assert_eq!(failure.stage, ImageStage::Unknown);
        assert_eq!(failure.error.code(), KitErrorCode::Timeout);

        // A large ancillary PNG chunk exercises independent limits without changing pixels.
        let mut png = base64::engine::general_purpose::STANDARD
            .decode(PNG)
            .unwrap();
        let at = png.len() - 12;
        let mut chunk = vec![0; 4 * 1024 * 1024 + 12];
        chunk[..4].copy_from_slice(&(4_u32 * 1024 * 1024).to_be_bytes());
        chunk[4..8].copy_from_slice(b"tEXt");
        png.splice(at..at, chunk);
        let image = ImagePart {
            mime: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(png),
        };
        assert!(crate::check_images(std::slice::from_ref(&image)).is_err());
        let mut request = input();
        request.images.push(image);
        let server = Server::script(vec![success()], Exhausted::Reply(Reply::close())).await;
        let limits = ImageLimits {
            max_image_bytes: 10 * 1024 * 1024,
            ..ImageLimits::default()
        };
        run(&connection(&server.url()), &request, &limits)
            .await
            .unwrap();
    }
}
