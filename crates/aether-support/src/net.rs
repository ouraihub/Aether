//! The only HTTP client constructor, plus the address policy that keeps aether
//! requests off private networks unless the user allowed a target (docs/10 §6, §11.4).

use std::error::Error as StdError;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::{Host, Url};

use crate::connection::RESERVED_HEADERS;
use crate::{KitError, KitErrorCode};

/// Adds custom `headers` to `builder`; call before setting system headers. Reserved
/// names are skipped again here, so system headers can never be doubled or replaced.
pub fn custom_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        if !RESERVED_HEADERS
            .iter()
            .any(|r| name.eq_ignore_ascii_case(r))
        {
            builder = builder.header(name, value);
        }
    }
    builder
}

/// Whether an explicit upstream response permits trying another configured key.
/// The caller must still enforce its request's pre-output retry boundary.
#[must_use]
pub fn next_key(error: &KitError) -> bool {
    matches!(error.details().status(), Some(401 | 403 | 429))
}

/// Maps a Native transport error without retaining a possibly secret-bearing URL.
pub fn transport(error: reqwest::Error) -> KitError {
    let error = error.without_url();
    if is_blocked(&error) {
        return KitError::bad_request("url", "blocked_address").with_cause(error);
    }
    let code = if error.is_timeout() {
        KitErrorCode::Timeout
    } else {
        KitErrorCode::Network
    };
    KitError::new(code, "upstream unreachable").with_cause(error)
}

/// A target the user explicitly allowed although it is private or plain http.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPort {
    /// Host name or IP literal (IPv6 without brackets); compared case-insensitively.
    pub host: String,
    /// The URL's explicit or scheme-default port.
    pub port: u16,
}

/// Which non-public targets aether may reach.
#[derive(Debug, Clone, Default)]
pub struct AddressPolicy {
    /// Private targets the user allowed one by one.
    pub allow_private: Vec<HostPort>,
}

impl AddressPolicy {
    fn allows(&self, host: &str, port: u16) -> bool {
        self.allow_private
            .iter()
            .any(|t| t.port == port && t.host.eq_ignore_ascii_case(host))
    }

    // ponytail: the resolver never sees the port, so at connect time any allowed port
    // of the host counts; check_url pins the port for http and IP literals.
    fn allows_host(&self, host: &str) -> bool {
        self.allow_private
            .iter()
            .any(|t| t.host.eq_ignore_ascii_case(host))
    }
}

/// Validates a URL before any request: `https`, or `http` to `localhost`, a loopback
/// literal or an allowed target; no userinfo; IP literals must be public unless
/// loopback or allowed. Domain names are vetted again at connect time by [`client`].
///
/// # Errors
/// `bad_request` on field `url` with reason `invalid_url`, `userinfo`, `scheme`,
/// `plain_http` or `blocked_address`.
pub fn check_url(url: &str, policy: &AddressPolicy) -> Result<Url, KitError> {
    let reject = |reason: &str| KitError::bad_request("url", reason);
    let parsed = Url::parse(url).map_err(|e| reject("invalid_url").with_cause(e))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(reject("userinfo"));
    }
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(reject("scheme"));
    }
    let (host, ip) = match parsed.host() {
        Some(Host::Domain(domain)) => (domain.to_owned(), None),
        Some(Host::Ipv4(ip)) => (ip.to_string(), Some(IpAddr::V4(ip))),
        Some(Host::Ipv6(ip)) => (ip.to_string(), Some(IpAddr::V6(ip))),
        None => return Err(reject("invalid_url")),
    };
    let allowed = policy.allows(&host, parsed.port_or_known_default().unwrap_or_default());
    let loopback = is_loopback(&parsed);
    if parsed.scheme() == "http" && !allowed && !loopback {
        return Err(reject("plain_http"));
    }
    // IP literals never reach the resolver, so they are vetted here.
    if ip.is_some_and(is_restricted) && !allowed && !loopback {
        return Err(reject("blocked_address"));
    }
    Ok(parsed)
}

/// Preflights a target URL and all locally resolved addresses before proxy submission.
/// This does not pin a third-party proxy's DNS or replace connect-time checks.
///
/// # Errors
/// URL policy errors, `network` on lookup failure or no addresses, or
/// `bad_request{url, blocked_address}` for a restricted address.
pub async fn check_target(url: &str, policy: &AddressPolicy) -> Result<Url, KitError> {
    let parsed = check_url(url, policy)?;
    if let Some(Host::Domain(host)) = parsed.host() {
        let port = parsed.port_or_known_default().unwrap_or_default();
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| {
                KitError::new(KitErrorCode::Network, "target lookup failed").with_cause(error)
            })?;
        let mut found = false;
        for address in addresses {
            found = true;
            let ip = address.ip();
            if is_restricted(ip)
                && !(is_localhost(host) && ip.to_canonical().is_loopback())
                && !policy.allows(host, port)
            {
                return Err(KitError::bad_request("url", "blocked_address"));
            }
        }
        if !found {
            return Err(KitError::new(
                KitErrorCode::Network,
                "target lookup returned no addresses",
            ));
        }
    }
    Ok(parsed)
}

/// Builds the client every aether module must use: rustls, no redirects, no proxy,
/// `aether/<ver>` User-Agent, and a resolver that vets every resolved address
/// at connect time. IP-literal hosts skip the resolver: pass URLs through
/// [`check_url`] first.
///
/// # Errors
/// `internal` if the TLS backend cannot initialise.
pub fn client(
    policy: AddressPolicy,
    connect_timeout: Duration,
) -> Result<reqwest::Client, KitError> {
    build(policy, connect_timeout, system_lookup)
}

/// Whether a request failed because the resolver refused an address.
#[must_use]
pub fn is_blocked(err: &reqwest::Error) -> bool {
    std::iter::successors(Some(err as &dyn StdError), |&e| e.source())
        .any(<dyn StdError>::is::<BlockedAddress>)
}

#[derive(Debug, thiserror::Error)]
#[error("address blocked by the address policy")]
struct BlockedAddress;

type LookupFuture = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;
type Lookup = fn(String) -> LookupFuture;

fn system_lookup(host: String) -> LookupFuture {
    Box::pin(async move { Ok(tokio::net::lookup_host((host.as_str(), 0)).await?.collect()) })
}

fn build(
    policy: AddressPolicy,
    connect_timeout: Duration,
    lookup: Lookup,
) -> Result<reqwest::Client, KitError> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .redirect(reqwest::redirect::Policy::none())
        // A proxy resolves the target itself and would defeat the connect-time check.
        .no_proxy()
        .user_agent(concat!("aether/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(connect_timeout)
        .dns_resolver(Arc::new(PolicyResolver {
            policy: Arc::new(policy),
            lookup,
        }))
        .build()
        .map_err(KitError::internal)
}

struct PolicyResolver {
    policy: Arc<AddressPolicy>,
    lookup: Lookup,
}

impl Resolve for PolicyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = Arc::clone(&self.policy);
        let lookup = self.lookup;
        Box::pin(async move {
            let host = name.as_str();
            let addrs = lookup(host.to_owned()).await?;
            let localhost = is_localhost(host);
            // One bad address rejects the whole answer: mixing a public and a private
            // address is the classic rebinding trick.
            let blocked = addrs.iter().any(|addr| {
                let ip = addr.ip();
                is_restricted(ip)
                    && !(localhost && ip.to_canonical().is_loopback())
                    && !policy.allows_host(host)
            });
            if blocked {
                return Err(Box::new(BlockedAddress) as Box<dyn StdError + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn is_localhost(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
}

/// Whether `url` targets `localhost` or a loopback IP literal.
#[must_use]
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => is_localhost(domain),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.to_canonical().is_loopback(),
        None => false,
    }
}

/// Private, link-local, loopback, multicast, unspecified or broadcast.
fn is_restricted(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_link_local()
                || v4.is_loopback()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    fn reason(url: &str, policy: &AddressPolicy) -> Option<String> {
        check_url(url, policy)
            .err()
            .map(|e| e.details().reason().unwrap_or_default().to_owned())
    }

    fn allow(host: &str, port: u16) -> AddressPolicy {
        AddressPolicy {
            allow_private: vec![HostPort {
                host: host.to_owned(),
                port,
            }],
        }
    }
    /// Answers every connection on a loopback port with `response`.
    async fn serve(response: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0_u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        port
    }

    const OK: &str = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";

    fn to_loopback(_host: String) -> LookupFuture {
        Box::pin(async { Ok(vec![SocketAddr::from(([127, 0, 0, 1], 0))]) })
    }

    fn to_public_and_private(_host: String) -> LookupFuture {
        Box::pin(async {
            Ok(vec![
                SocketAddr::from(([93, 184, 216, 34], 0)),
                SocketAddr::from(([10, 0, 0, 5], 0)),
            ])
        })
    }

    #[tokio::test]
    async fn target_preflight_keeps_url_policy_and_localhost_exception() {
        let none = AddressPolicy::default();
        assert_eq!(
            check_target("https://10.0.0.5/page", &none)
                .await
                .unwrap_err()
                .details()
                .reason(),
            Some("blocked_address")
        );
        assert_eq!(
            check_target("https://name:secret@localhost/page", &none)
                .await
                .unwrap_err()
                .details()
                .reason(),
            Some("userinfo")
        );
        let local = check_target("http://localhost:8080/page", &none)
            .await
            .unwrap();
        assert_eq!(local.port(), Some(8080));
        let permitted = check_target("http://10.0.0.5:8080/page", &allow("10.0.0.5", 8080))
            .await
            .unwrap();
        assert_eq!(permitted.path(), "/page");
        assert!(
            check_target("http://10.0.0.5:8081/page", &allow("10.0.0.5", 8080))
                .await
                .is_err()
        );
    }

    #[test]
    fn check_url_allows_https_and_loopback_http() {
        let none = AddressPolicy::default();
        for url in [
            "https://api.example.com/v1",
            "http://localhost:3000",
            "http://LOCALHOST:3000/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            assert_eq!(reason(url, &none), None, "{url}");
        }
    }

    #[test]
    fn check_url_rejects_unsafe_urls() {
        let none = AddressPolicy::default();
        for (url, expected) in [
            ("http://192.168.1.2", "plain_http"),
            ("https://192.168.1.2", "blocked_address"),
            ("https://10.0.0.1/v1", "blocked_address"),
            ("https://169.254.169.254/latest", "blocked_address"),
            ("https://[fe80::1]/", "blocked_address"),
            ("https://[::ffff:192.168.1.2]/", "blocked_address"),
            ("https://0.0.0.0/", "blocked_address"),
            ("http://api.example.com", "plain_http"),
            ("https://user:pw@api.example.com", "userinfo"),
            ("http://user@localhost:3000", "userinfo"),
            ("ftp://api.example.com", "scheme"),
            ("not a url", "invalid_url"),
        ] {
            assert_eq!(reason(url, &none).as_deref(), Some(expected), "{url}");
        }
    }

    #[test]
    fn allow_private_admits_exactly_that_host_and_port() {
        let policy = allow("192.168.1.2", 80);
        assert_eq!(reason("http://192.168.1.2", &policy), None);
        assert_eq!(reason("http://192.168.1.2:80/v1", &policy), None);
        assert_eq!(
            reason("http://192.168.1.2:8080", &policy).as_deref(),
            Some("plain_http")
        );
        assert_eq!(
            reason("http://192.168.1.3", &policy).as_deref(),
            Some("plain_http")
        );
        let named = allow("NAS.lan", 8080);
        assert_eq!(reason("http://nas.lan:8080", &named), None);
    }

    #[tokio::test]
    async fn localhost_http_reaches_a_loopback_server() {
        let port = serve(OK).await;
        let url = check_url(
            &format!("http://localhost:{port}"),
            &AddressPolicy::default(),
        )
        .unwrap();
        let client = client(AddressPolicy::default(), Duration::from_secs(5)).unwrap();
        let response = client.get(url).send().await.unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn public_domain_resolving_to_loopback_is_rejected_at_connect() {
        let port = serve(OK).await;
        let url = format!("https://evil.example:{port}/");
        // The URL string alone looks fine; only the resolved address gives it away.
        assert_eq!(reason(&url, &AddressPolicy::default()), None);
        let client = build(
            AddressPolicy::default(),
            Duration::from_secs(5),
            to_loopback,
        )
        .unwrap();
        let err = client.get(&url).send().await.unwrap_err();
        assert!(is_blocked(&err), "{err:?}");
    }

    #[tokio::test]
    async fn one_private_address_in_the_answer_rejects_it() {
        let client = build(
            AddressPolicy::default(),
            Duration::from_secs(5),
            to_public_and_private,
        )
        .unwrap();
        let err = client
            .get("https://rebind.example/")
            .send()
            .await
            .unwrap_err();
        assert!(is_blocked(&err), "{err:?}");
    }

    #[tokio::test]
    async fn allowed_private_domain_connects() {
        let port = serve(OK).await;
        let policy = allow("intranet.test", port);
        let url = check_url(&format!("http://intranet.test:{port}"), &policy).unwrap();
        let client = build(policy, Duration::from_secs(5), to_loopback).unwrap();
        let response = client.get(url).send().await.unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let port = serve(
            "HTTP/1.1 302 Found\r\nlocation: /elsewhere\r\n\
content-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await;
        let client = client(AddressPolicy::default(), Duration::from_secs(5)).unwrap();
        let response = client
            .get(format!("http://127.0.0.1:{port}/start"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302);
        assert_eq!(response.url().path(), "/start");
    }
}
