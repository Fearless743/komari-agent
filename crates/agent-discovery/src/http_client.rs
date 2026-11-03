//! `reqwest` integration for the custom DNS resolver.
//!
//! `reqwest` exposes a `dns::Resolve` hook. Implementing it keeps DNS
//! resolution inside the connection layer, so one client can retry all
//! addresses without re-sending a non-idempotent request, and redirects also
//! use the custom resolver.

use std::net::SocketAddr;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::dns::DnsResolver;

/// Adapter from [`DnsResolver`] to reqwest's resolver hook.
#[derive(Clone)]
pub struct DnsReqwestResolver {
    resolver: DnsResolver,
}

impl DnsReqwestResolver {
    pub fn new(resolver: DnsResolver) -> Self {
        Self { resolver }
    }
}

impl Resolve for DnsReqwestResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.resolver.clone();
        let host = name.as_str().to_owned();

        Box::pin(async move {
            let addresses = resolver
                .resolve_host(&host)
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?;

            let addresses: Vec<SocketAddr> = addresses
                .into_iter()
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();

            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// Build a `reqwest` client that resolves every host through [`DnsResolver`].
///
/// The returned client can be passed to `agent_update::ReqwestUpdateHttp::with_client`
/// so update checks and downloads use `custom_dns` and the configured IP version
/// preference, including redirect targets.
pub fn build_reqwest_client(
    resolver: DnsResolver,
    timeout: Duration,
    ignore_unsafe_cert: bool,
) -> Result<reqwest::Client, reqwest::Error> {
    let timeout = if timeout.is_zero() {
        Duration::from_secs(30)
    } else {
        timeout
    };

    reqwest::Client::builder()
        .dns_resolver(DnsReqwestResolver::new(resolver))
        .timeout(timeout)
        .danger_accept_invalid_certs(ignore_unsafe_cert)
        .user_agent(format!("komari-agent-rs/{}", env!("CARGO_PKG_VERSION")))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::{PreferIp, ResolverConfig};

    #[test]
    fn builds_client_with_custom_resolver() {
        let resolver = DnsResolver::new(ResolverConfig {
            custom_dns: Some("127.0.0.1:5353".to_owned()),
            prefer_ip: PreferIp::V4,
            ..ResolverConfig::default()
        })
        .unwrap();

        assert!(build_reqwest_client(resolver, Duration::from_secs(5), false).is_ok());
    }
}
