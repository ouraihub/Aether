//! `generate_image` tool: spec and an OpenAI-compatible `/images/generations` and
//! `/images/edits` client (docs/20, docs/37).

use std::time::Duration;

use crate::run::{next_key, transport};
use crate::types::{Approval, ImagePart, MAX_IMAGE_BYTES, ToolSource, ToolSpec};
use aether_support::net::{self, AddressPolicy};
use aether_support::{Connection, KitError};
use base64::Engine as _;
use serde_json::{Value, json};

/// Tool name the chat model calls.
pub const GENERATE_IMAGE: &str = "generate_image";
/// Images one edit call may reference (docs/37 §2).
pub const MAX_EDIT_IMAGES: usize = 4;
/// Entries of the `image_ids` choice list (docs/37 §2.1).
pub const MAX_IMAGE_CHOICES: usize = 8;
const SIZES: [&str; 3] = ["1024x1024", "1024x1536", "1536x1024"];
const TIMEOUT: Duration = Duration::from_secs(120);

/// Where an `image_ids` choice comes from; choice `i` has id `image-{i+1}` (docs/37 §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageOrigin {
    /// Attached by the user in this message.
    Attached,
    /// Attached by the user in an earlier message.
    EarlierInput,
    /// Generated in an earlier reply.
    Generated,
}

/// Catalogue entry; added only when the host turned image generation on (docs/20 §2).
/// Non-empty `choices` (newest first, at most [`MAX_IMAGE_CHOICES`]) add `image_ids`.
#[must_use]
pub fn generate_image_spec(choices: &[ImageOrigin]) -> ToolSpec {
    let mut description = "Generate one image from a detailed English or Chinese prompt. Use only when the user asks for a picture, drawing, illustration or photo.".to_owned();
    let mut properties = json!({
        "prompt": { "type": "string", "minLength": 1, "maxLength": 4000, "description": "What to draw." },
        "size": { "type": "string", "enum": SIZES }
    });
    if !choices.is_empty() {
        description.push_str(" To edit or use existing images as reference, pass their ids in image_ids; omit it to draw a new image.\nImages you can pass in image_ids (newest first):");
        let ids: Vec<String> = (1..=choices.len()).map(|i| format!("image-{i}")).collect();
        for (id, origin) in ids.iter().zip(choices) {
            let what = match origin {
                ImageOrigin::Attached => "attached by the user in this message",
                ImageOrigin::EarlierInput => "attached in an earlier message",
                ImageOrigin::Generated => "generated in an earlier reply",
            };
            description.push_str(&format!("\n{id}: {what}"));
        }
        properties["image_ids"] = json!({
            "type": "array",
            "items": { "type": "string", "enum": ids },
            "minItems": 1,
            "maxItems": MAX_EDIT_IMAGES,
            "description": "Existing images to edit or use as reference."
        });
    }
    ToolSpec {
        name: GENERATE_IMAGE.to_owned(),
        description,
        parameters: json!({
            "type": "object",
            "properties": properties,
            "required": ["prompt"],
            "additionalProperties": false
        }),
        approval: Approval::Auto,
        source: ToolSource::Host,
    }
}

/// Validates the model's arguments and calls the painting model once: `/images/generations`
/// without `images`, multipart `/images/edits` with them (resolved from `image_ids` by the host).
///
/// # Errors
/// `bad_request{prompt|size}` for invalid arguments; `bad_request{images, encoding}` for
/// undecodable input images; `network`/upstream errors from the call;
/// `bad_request{images, size}` when the returned image exceeds [`MAX_IMAGE_BYTES`].
pub async fn generate_image(
    painting: &Connection,
    policy: &AddressPolicy,
    args: &Value,
    images: &[ImagePart],
) -> Result<ImagePart, KitError> {
    let prompt = args["prompt"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.chars().count() <= 4000);
    let prompt = prompt.ok_or_else(|| KitError::bad_request("prompt", "invalid"))?;
    let size = match args.get("size") {
        None | Some(Value::Null) => SIZES[0],
        Some(value) => SIZES
            .into_iter()
            .find(|s| value.as_str() == Some(*s))
            .ok_or_else(|| KitError::bad_request("size", "invalid"))?,
    };
    let (base, path) = if images.is_empty() {
        (painting.image_base_url.as_deref(), "generations")
    } else {
        (painting.image_edit_base_url.as_deref(), "edits")
    };
    let url = net::check_url(
        &format!(
            "{}/images/{path}",
            base.unwrap_or(&painting.base_url).trim_end_matches('/')
        ),
        policy,
    )?;
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let request = aether_support::connection::custom_headers(client.post(url), &painting.headers)
        .timeout(TIMEOUT);
    // No `response_format`: gpt-image-* rejects it (always b64); others may answer `url`, handled below.
    let request = if images.is_empty() {
        request.json(&json!({ "model": painting.model_id, "prompt": prompt, "size": size, "n": 1 }))
    } else {
        let boundary = boundary()?;
        let model = painting.model_id.as_deref().unwrap_or_default();
        let body = multipart(
            &boundary,
            &[
                ("model", model),
                ("prompt", prompt),
                ("size", size),
                ("n", "1"),
            ],
            images,
        )?;
        request
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
    };
    let known: Vec<&str> = painting.api_keys.iter().map(|key| key.as_str()).collect();
    let attempts = known.len().max(1);
    let mut attempt = 0;
    // Same key failover as a chat round (docs/34 §3.4).
    let (status, body) = loop {
        // In-memory bodies clone cheaply (shared bytes).
        let mut request = request
            .try_clone()
            .ok_or_else(|| KitError::internal(std::io::Error::other("unclonable request")))?;
        if let Some(key) = known.get(attempt) {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(transport)?;
        let status = response.status();
        let body: Value = response.json().await.map_err(transport)?;
        if status.is_success() {
            break (status, body);
        }
        let text = serde_json::to_string(&body).unwrap_or_default();
        let error = KitError::upstream(status.as_u16(), &text, &known);
        if attempt + 1 < attempts && next_key(&error) {
            attempt += 1;
        } else {
            return Err(error);
        }
    };
    let first = &body["data"][0];
    let bytes = if let Some(data) = first["b64_json"].as_str() {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| KitError::bad_request("images", "encoding"))?
    } else if let Some(link) = first["url"].as_str() {
        let url = net::check_url(link, policy)?;
        client
            .get(url)
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(transport)?
            .bytes()
            .await
            .map_err(transport)?
            .to_vec()
    } else {
        return Err(KitError::upstream(
            status.as_u16(),
            "no image in response",
            &[],
        ));
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(KitError::bad_request("images", "size"));
    }
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "image/png"
    };
    Ok(ImagePart {
        mime: mime.to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// A random multipart boundary.
fn boundary() -> Result<String, KitError> {
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random)
        .map_err(|e| KitError::internal(std::io::Error::other(e.to_string())))?;
    Ok(format!(
        "aether-{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}

/// Hand-written `multipart/form-data` body (no reqwest `multipart` feature): text `fields`,
/// then `image` (one) or `image[]` (each of several) as `image-N.<ext>` with its MIME.
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
    for (i, image) in images.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| KitError::bad_request("images", "encoding"))?;
        let ext = match image.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"image-{}.{ext}\"\r\nContent-Type: {}\r\n\r\n",
                i + 1,
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
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
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
            can_vision: false,
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
        let image = generate_image(
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
            generate_image(&painting(&base), &policy, &json!({"prompt": "x"}), &[])
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
        let error = generate_image(&painting(&base), &policy, &json!({"prompt": "x"}), &[])
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
                generate_image(&painting("http://127.0.0.1:9"), &policy, &args, &[])
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
        generate_image(&connection, &policy, &json!({"prompt": "x"}), &[])
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
        generate_image(&connection, &policy, &json!({"prompt": "x"}), &[])
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
    fn spec_offers_image_ids_only_with_choices() {
        let plain = generate_image_spec(&[]);
        assert!(plain.parameters["properties"].get("image_ids").is_none());
        assert!(!plain.description.contains("image_ids"));
        let spec = generate_image_spec(&[ImageOrigin::Attached, ImageOrigin::Generated]);
        let ids = &spec.parameters["properties"]["image_ids"];
        assert_eq!(ids["items"]["enum"], json!(["image-1", "image-2"]));
        assert_eq!(
            (ids["minItems"].as_u64(), ids["maxItems"].as_u64()),
            (Some(1), Some(4))
        );
        assert!(spec.description.contains(
            "image-1: attached by the user in this message\nimage-2: generated in an earlier reply"
        ));
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
        generate_image(&connection, &policy, &json!({"prompt": "blue"}), &input)
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
        generate_image(&connection, &policy, &json!({"prompt": "x"}), &input)
            .await
            .unwrap();
        assert!(seen.lock().await[0].starts_with("POST /v1/images/edits "));
    }
}
