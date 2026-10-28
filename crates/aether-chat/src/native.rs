//! Concrete Native model gateway: headers, HTTP and SSE transport only.

use crate::Usage;
use crate::protocol::{Decoded, Decoder, Description, Round, path};
use aether_support::{
    KitError, KitErrorCode, Protocol,
    net::{self, AddressPolicy},
    sse::SseParser,
};
use std::time::Duration;

pub(crate) struct ModelGateway {
    http: reqwest::Client,
    policy: AddressPolicy,
}

impl ModelGateway {
    pub(crate) fn new(policy: AddressPolicy) -> Result<Self, KitError> {
        Ok(Self {
            http: net::client(policy.clone(), Duration::from_secs(15))?,
            policy,
        })
    }

    pub(crate) async fn stream(
        &self,
        base: &str,
        request: Description<'_>,
        known: &[&str],
        (first_byte, idle): (Duration, Duration),
        usage: &mut Usage,
        mut on: impl FnMut(Decoded) -> Result<(), KitError>,
    ) -> Result<Round, KitError> {
        let url = net::check_url(
            &format!("{}/{}", base.trim_end_matches('/'), path(request.protocol)),
            &self.policy,
        )?;
        // Serialize the pure description once, before any credential rotation.
        let builder = net::custom_headers(self.http.post(url), request.headers)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&request.body);
        let mut attempt = 0;
        let (mut response, mut chunk) = loop {
            let mut builder = builder.try_clone().ok_or_else(|| {
                KitError::internal(std::io::Error::other("unclonable model request"))
            })?;
            let key = known.get(attempt).copied();
            if let Some(key) = key {
                builder = builder.bearer_auth(key);
            }
            if request.protocol == Protocol::AnthropicMessages {
                if let Some(key) = key {
                    builder = builder.header("x-api-key", key);
                }
                builder = builder.header("anthropic-version", "2023-06-01");
            }
            let started = async {
                let mut response = builder.send().await.map_err(net::transport)?;
                if !response.status().is_success() {
                    return Err(upstream(response, known).await);
                }
                let chunk = response.chunk().await.map_err(net::transport)?;
                Ok((response, chunk))
            };
            match tokio::time::timeout(first_byte, started)
                .await
                .map_err(|_| timed_out("first_byte"))?
            {
                Ok(started) => break started,
                Err(error) if attempt + 1 < known.len() && net::next_key(&error) => attempt += 1,
                Err(error) => return Err(error),
            }
        };
        let mut parser = SseParser::new();
        let mut decoder = Decoder::new(request.protocol);
        while let Some(bytes) = chunk {
            for event in parser.feed(&bytes) {
                on(decoder.event(&event.data, usage))?;
            }
            chunk = tokio::time::timeout(idle, response.chunk())
                .await
                .map_err(|_| timed_out("idle"))?
                .map_err(net::transport)?;
        }
        for event in parser.finish() {
            on(decoder.event(&event.data, usage))?;
        }
        if decoder.done {
            Ok(decoder.finish())
        } else {
            Err(KitError::new(KitErrorCode::Network, "stream ended early")
                .with_reason("incomplete_stream"))
        }
    }
}

fn timed_out(reason: &str) -> KitError {
    KitError::new(KitErrorCode::Timeout, format!("upstream {reason} timeout")).with_reason(reason)
}

async fn upstream(mut response: reqwest::Response, known: &[&str]) -> KitError {
    const LIMIT: usize = 8192;
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while body.len() < LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk[..chunk.len().min(LIMIT - body.len())])
            }
            Ok(None) | Err(_) => break,
        }
    }
    KitError::upstream(status, &String::from_utf8_lossy(&body), known)
}
