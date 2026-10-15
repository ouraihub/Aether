//! `generate_image` tool: spec and an OpenAI-compatible `/images/generations` client (docs/20).

use std::time::Duration;

use crate::run::transport;
use crate::types::{Approval, ImagePart, MAX_IMAGE_BYTES, ToolSource, ToolSpec};
use aether_support::net::{self, AddressPolicy};
use aether_support::{Connection, KitError};
use base64::Engine as _;
use serde_json::{Value, json};

/// Tool name the chat model calls.
pub const GENERATE_IMAGE: &str = "generate_image";
const SIZES: [&str; 3] = ["1024x1024", "1024x1536", "1536x1024"];
const TIMEOUT: Duration = Duration::from_secs(120);

/// Catalogue entry; added only when the host turned image generation on (docs/20 §2).
#[must_use]
pub fn generate_image_spec() -> ToolSpec {
    ToolSpec {
        name: GENERATE_IMAGE.to_owned(),
        description: "Generate one image from a detailed English or Chinese prompt. Use only when the user asks for a picture, drawing, illustration or photo.".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "minLength": 1, "maxLength": 4000, "description": "What to draw." },
                "size": { "type": "string", "enum": SIZES }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
        approval: Approval::Auto,
        source: ToolSource::Host,
    }
}

/// Validates the model's arguments and calls the painting model once.
///
/// # Errors
/// `bad_request{prompt|size}` for invalid arguments; `network`/upstream errors from the call;
/// `bad_request{images, size}` when the returned image exceeds [`MAX_IMAGE_BYTES`].
pub async fn generate_image(
    painting: &Connection,
    policy: &AddressPolicy,
    args: &Value,
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
    let url = net::check_url(
        &format!(
            "{}/images/generations",
            painting.base_url.trim_end_matches('/')
        ),
        policy,
    )?;
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let mut request = client
        .post(url)
        .timeout(TIMEOUT)
        .json(&json!({ "model": painting.model_id, "prompt": prompt, "size": size, "n": 1, "response_format": "b64_json" }));
    if let Some(key) = &painting.api_key {
        request = request.bearer_auth(key.as_str());
    }
    let response = request.send().await.map_err(transport)?;
    let status = response.status();
    let body: Value = response.json().await.map_err(transport)?;
    if !status.is_success() {
        let text = serde_json::to_string(&body).unwrap_or_default();
        let secret = painting.api_key.as_deref().map(String::as_str);
        return Err(KitError::upstream(
            status.as_u16(),
            &text,
            secret.as_slice(),
        ));
    }
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
                log.lock()
                    .await
                    .push(raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned());
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
            api_key: Some(Zeroizing::new("sk-paint".to_owned())),
            model_id: Some("gpt-image-1".to_owned()),
            endpoint_id: None,
            context_window: None,
            max_output: None,
            supports_tools: None,
            reasoning: None,
            native_web_search: false,
            can_vision: false,
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
        let sent: Value = serde_json::from_str(&seen.lock().await[0]).unwrap();
        assert_eq!(
            sent,
            json!({"model": "gpt-image-1", "prompt": "a red fox", "size": "1536x1024", "n": 1, "response_format": "b64_json"})
        );

        let (files, _) = serve(vec![(200, "\u{89}PNG".to_owned())]).await;
        let (base, _) = serve(vec![(
            200,
            format!(r#"{{"data":[{{"url":"{files}/a.png"}}]}}"#),
        )])
        .await;
        assert_eq!(
            generate_image(&painting(&base), &policy, &json!({"prompt": "x"}))
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
        let error = generate_image(&painting(&base), &policy, &json!({"prompt": "x"}))
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
                generate_image(&painting("http://127.0.0.1:9"), &policy, &args)
                    .await
                    .unwrap_err()
                    .code(),
                aether_support::KitErrorCode::BadRequest
            );
        }
    }
}
