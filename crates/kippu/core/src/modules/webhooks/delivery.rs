//! The HTTP side of delivering to webhooks, and the policy on which URLs may be reached.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;

use crate::config::WebhooksConfig;

/// Whether an address belongs to the public internet: not loopback, private, link-local
/// (which includes cloud metadata services at 169.254.169.254), shared, documentation,
/// multicast or otherwise reserved.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
                || (a == 192 && b == 0 && v4.octets()[2] == 0) // IETF protocol assignments
                || (a == 198 && (18..20).contains(&b)) // benchmarking
                || a >= 240) // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local
                || (first & 0xffc0) == 0xfe80 // link-local
                || first == 0x2001 && v6.segments()[1] == 0x0db8 // documentation
                || first == 0x0064 && v6.segments()[1] == 0xff9b) // NAT64, may reach IPv4 private space
        }
    }
}

/// Why a URL may not be delivered to, or `None` if it may.
pub(crate) fn refusal(url: &str, config: &WebhooksConfig) -> Option<&'static str> {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Some("is not a valid URL");
    };
    match parsed.scheme() {
        "https" => {}
        "http" if config.allow_http => {}
        "http" => return Some("must use https"),
        _ => return Some("must be an http(s) URL"),
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Some("must not contain credentials");
    }
    let Some(host) = parsed.host_str() else {
        return Some("must name a host");
    };
    let host_is_private = if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        !is_public(ip)
    } else {
        let domain = host.trim_end_matches('.');
        domain == "localhost" || domain.ends_with(".localhost")
    };
    (host_is_private && !config.allow_private_networks)
        .then_some("must not point to a loopback or private address")
}

/// Resolves names but refuses to hand out non-public addresses, so a hostname that resolves
/// into the private network (or is re-pointed there later) cannot be reached either.
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| is_public(address.ip()))
                .collect();
            if addresses.is_empty() {
                return Err(format!("{host} resolves to no public address").into());
            }
            let addresses: Addrs = Box::new(addresses.into_iter());
            Ok(addresses)
        })
    }
}

/// Builds the HTTP client deliveries go through: TLS with the system's roots, no redirects
/// (a redirect is a failure), no proxy from the environment, and the address policy above.
pub(crate) fn client(config: &WebhooksConfig) -> Result<reqwest::Client, String> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_native_certs::load_native_certs().certs {
        // A certificate the platform ships but rustls cannot parse is skipped, not fatal.
        let _ = roots.add(certificate);
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| error.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let mut builder = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(Policy::none())
        .timeout(Duration::from_secs(config.timeout_seconds.get()))
        .user_agent(concat!("kippu-webhooks/", env!("CARGO_PKG_VERSION")));
    if !config.allow_private_networks {
        builder = builder.dns_resolver(Arc::new(PublicOnly));
    }
    builder.build().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_reserved_addresses_are_not_public() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["93.184.215.14", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn urls_follow_the_deployment_policy() {
        let strict = WebhooksConfig::default();
        assert_eq!(refusal("https://hooks.example.org/kippu", &strict), None);
        assert_eq!(
            refusal("http://hooks.example.org", &strict),
            Some("must use https")
        );
        for internal in [
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://169.254.169.254/latest/meta-data",
            "https://localhost:8080/",
            "https://admin.localhost/",
        ] {
            assert!(refusal(internal, &strict).is_some(), "{internal}");
        }
        assert!(refusal("https://user:pw@hooks.example.org", &strict).is_some());
        let lax = WebhooksConfig {
            allow_http: true,
            allow_private_networks: true,
            ..WebhooksConfig::default()
        };
        assert_eq!(refusal("http://127.0.0.1:9000/hook", &lax), None);
    }
}
