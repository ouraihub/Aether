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
        request.json(&json!({"model": connection.model_id, "prompt": prompt, "size": input.size.as_str(), "n": 1}))
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
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"image-{}.{ext}\"\r\nContent-Type: {}\r\n\r\n", index + 1, image.mime).as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(body)
}
#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "a failed assertion is a failed test")]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use aether_support::{Protocol, net::AddressPolicy};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use zeroize::Zeroizing;

    use super::*;

    async fn generate_for_test(
        connection: &Connection,
        policy: &AddressPolicy,
        args: &Value,
        images: &[ImagePart],
    ) -> Result<ImagePart, KitError> {
        let size = match args.get("size").and_then(Value::as_str) {
            None | Some("1024x1024") => crate::ImageSize::Square,
            Some("1024x1536") => crate::ImageSize::Portrait,
            Some("1536x1024") => crate::ImageSize::Landscape,
            Some(_) => return Err(KitError::bad_request("size", "invalid")),
        };
        generate(
            connection,
            policy,
            &ImageRequest {
                prompt: args["prompt"].as_str().unwrap_or_default().to_owned(),
                size,
                images: images.to_vec(),
            },
            &CancellationToken::new(),
        )
        .await
    }

    const PNG: &str = "iVBORw0KGgo=";

    /// Answers each request with the next `(status, body)` and records request bodies.
    async fn serve(replies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0_u8; 65_536];
                let n = socket.read(&mut buf).await.unwrap();
                let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
                log.lock().await.push(raw);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.chars().count()
                );
                let mut reply = reply.into_bytes();
                reply.extend(body.chars().map(|ch| ch as u8));
                socket.write_all(&reply).await.unwrap();
            }
        });
        (base, seen)
    }

    fn painting(base: &str) -> Connection {
        Connection {
            base_url: base.to_owned(),
            protocol: Protocol::OpenaiChat,
            api_keys: vec![Zeroizing::new("sk-paint".to_owned())],
            model_id: Some("gpt-image-1".to_owned()),
            endpoint_id: None,
            context_window: None,
            max_output: None,
            supports_tools: None,
            reasoning: None,
            native_web_search: false,
            pricing: None,
            can_vision: true,
            headers: Vec::new(),
            image_base_url: None,
            image_edit_base_url: None,
        }
    }

    #[tokio::test]
    async fn paints_from_b64_and_url_and_reports_upstream_errors() {
        let policy = AddressPolicy::default();
        let (base, seen) = serve(vec![(
            200,
            format!(r#"{{"data":[{{"b64_json":"{PNG}"}}]}}"#),
        )])
        .await;
        let image = generate_for_test(
            &painting(&base),
            &policy,
            &json!({"prompt": "a red fox", "size": "1536x1024"}),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(
            image,
            ImagePart {
                mime: "image/png".to_owned(),
                data: PNG.to_owned()
            }
        );
        let sent = seen.lock().await[0].clone();
        let sent: Value = serde_json::from_str(sent.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"model": "gpt-image-1", "prompt": "a red fox", "size": "1536x1024", "n": 1})
        );

        let (files, _) = serve(vec![(200, "\u{89}PNG".to_owned())]).await;
        let (base, _) = serve(vec![(
            200,
            format!(r#"{{"data":[{{"url":"{files}/a.png"}}]}}"#),
        )])
        .await;
        assert_eq!(
            generate_for_test(&painting(&base), &policy, &json!({"prompt": "x"}), &[])
                .await
                .unwrap()
                .mime,
            "image/png"
        );

        let (base, _) = serve(vec![(
            400,
            r#"{"error":{"message":"bad prompt sk-paint"}}"#.to_owned(),
        )])
        .await;
        let error = generate_for_test(&painting(&base), &policy, &json!({"prompt": "x"}), &[])
            .await
            .unwrap_err();
        assert!(
            !error.message().contains("sk-paint"),
            "key must be redacted: {}",
            error.message()
        );

        for args in [
            json!({}),
            json!({"prompt": "  "}),
            json!({"prompt": "x", "size": "9x9"}),
        ] {
            assert_eq!(
                generate_for_test(&painting("http://127.0.0.1:9"), &policy, &args, &[])
                    .await
                    .unwrap_err()
                    .code(),
                aether_support::KitErrorCode::BadRequest
            );
        }
    }

    #[tokio::test]
    async fn rate_limited_key_fails_over_to_the_next() {
        let policy = AddressPolicy::default();
        let ok = format!(r#"{{"data":[{{"b64_json":"{PNG}"}}]}}"#);
        let (base, seen) = serve(vec![(429, "{}".to_owned()), (200, ok)]).await;
        let mut connection = painting(&base);
        connection
            .api_keys
            .push(Zeroizing::new("sk-paint-2".to_owned()));
        generate_for_test(&connection, &policy, &json!({"prompt": "x"}), &[])
            .await
            .unwrap();
        assert_eq!(seen.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn image_base_url_and_custom_headers_are_used() {
        let policy = AddressPolicy::default();
        let ok = format!(r#"{{"data":[{{"b64_json":"{PNG}"}}]}}"#);
        let (image_base, seen) = serve(vec![(200, ok)]).await;
        let mut connection = painting("http://127.0.0.1:9");
        connection.image_base_url = Some(format!("{image_base}/v1"));
        connection.headers = vec![("X-Relay-Tag".to_owned(), "team-a".to_owned())];
        generate_for_test(&connection, &policy, &json!({"prompt": "x"}), &[])
            .await
            .unwrap();
        let raw = seen.lock().await[0].to_ascii_lowercase();
        assert!(raw.starts_with("post /v1/images/generations "), "{raw}");
        assert!(raw.contains("x-relay-tag: team-a"), "{raw}");
        assert!(raw.contains("authorization: bearer sk-paint"), "{raw}");
    }

    fn part(mime: &str, bytes: &[u8]) -> ImagePart {
        ImagePart {
            mime: mime.to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[test]
    fn multipart_names_one_image_and_several_images() {
        let fields = [
            ("model", "m"),
            ("prompt", "p"),
            ("size", "1024x1024"),
            ("n", "1"),
        ];
        let one = multipart("B", &fields, &[part("image/jpeg", b"JPG")]).unwrap();
        let one = String::from_utf8(one).unwrap();
        for (name, value) in fields {
            assert!(
                one.contains(&format!("name=\"{name}\"\r\n\r\n{value}\r\n")),
                "{one}"
            );
        }
        assert!(one.contains(
            "name=\"image\"; filename=\"image-1.jpg\"\r\nContent-Type: image/jpeg\r\n\r\nJPG\r\n"
        ));
        assert!(one.ends_with("--B--\r\n") && !one.contains("image[]"));

        let two = multipart(
            "B",
            &fields,
            &[part("image/png", b"P1"), part("image/webp", b"W2")],
        )
        .unwrap();
        let two = String::from_utf8(two).unwrap();
        assert_eq!(two.matches("filename=").count(), 2);
        assert_eq!(two.matches("name=\"image[]\"").count(), 2);
        assert!(two.contains("filename=\"image-1.png\"\r\nContent-Type: image/png\r\n\r\nP1"));
        assert!(two.contains("filename=\"image-2.webp\"\r\nContent-Type: image/webp\r\n\r\nW2"));
    }

    #[tokio::test]
    async fn edits_post_multipart_with_headers_auth_failover_and_url_fallback() {
        let policy = AddressPolicy::default();
        let ok = format!(r#"{{"data":[{{"b64_json":"{PNG}"}}]}}"#);
        let input = [part("image/png", b"\x89PNG")];
        // Edit base URL set: used, with custom headers first and key failover.
        let (edit_base, seen) = serve(vec![(429, "{}".to_owned()), (200, ok.clone())]).await;
        let mut connection = painting("http://127.0.0.1:9");
        connection.image_edit_base_url = Some(format!("{edit_base}/v1"));
        connection.headers = vec![("X-Relay-Tag".to_owned(), "team-a".to_owned())];
        connection
            .api_keys
            .push(Zeroizing::new("sk-paint-2".to_owned()));
        generate_for_test(&connection, &policy, &json!({"prompt": "blue"}), &input)
            .await
            .unwrap();
        let seen = seen.lock().await;
        assert_eq!(seen.len(), 2);
        let raw = seen[1].to_ascii_lowercase();
        assert!(raw.starts_with("post /v1/images/edits "), "{raw}");
        assert!(raw.contains("x-relay-tag: team-a"), "{raw}");
        assert!(raw.contains("authorization: bearer sk-paint-2"), "{raw}");
        assert!(
            raw.contains("content-type: multipart/form-data; boundary=aether-"),
            "{raw}"
        );
        assert!(!raw.contains("application/json\r\n"), "{raw}");
        drop(seen);

        // No edit base URL: falls back to `base_url`, never to the generation base URL.
        let (base, seen) = serve(vec![(200, ok)]).await;
        let mut connection = painting(&format!("{base}/v1"));
        connection.image_base_url = Some("http://127.0.0.1:9".to_owned());
        generate_for_test(&connection, &policy, &json!({"prompt": "x"}), &input)
            .await
            .unwrap();
        assert!(seen.lock().await[0].starts_with("POST /v1/images/edits "));
    }
}
