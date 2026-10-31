#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "a failed assertion is a failed test")]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use aether_support::{net::AddressPolicy, Protocol};
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

