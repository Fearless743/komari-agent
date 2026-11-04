//! `reqwest` integration for the custom DNS resolver.
//!
//! `reqwest` exposes a `dns::Resolve` hook. Implementing it keeps DNS
//! resolution inside the connection layer, so one client can retry all
//! addresses without re-sending a non-idempotent request, and redirects also
//! use the custom resolver.

use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use thiserror::Error;

use crate::dns::{DnsError, DnsResolver};

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

/// An HTTP request understood by [`HttpClient`].
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// An HTTP response returned by [`HttpClient`].
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Injectable HTTP transport.
///
/// The default implementation is [`ReqwestHttpClient`], which resolves the
/// endpoint with [`DnsResolver`] and tries each address in preference order.
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, DiscoveryError>;
}

/// `reqwest` transport using the custom DNS resolver.
///
/// A single client is used for all addresses; reqwest/hyper handles address
/// attempts inside the connector.
pub struct ReqwestHttpClient {
    client: reqwest::Client,
    timeout: Duration,
}

impl ReqwestHttpClient {
    /// Build a client with the custom resolver and an overall request timeout.
    pub fn new(
        resolver: DnsResolver,
        timeout: Duration,
        ignore_unsafe_cert: bool,
    ) -> Result<Self, DiscoveryError> {
        let timeout = if timeout.is_zero() {
            Duration::from_secs(30)
        } else {
            timeout
        };

        let client = build_reqwest_client(resolver, timeout, ignore_unsafe_cert)
            .map_err(|err| DiscoveryError::Http(format!("failed to build HTTP client: {err}")))?;

        Ok(Self { client, timeout })
    }

    async fn send_once(
        &self,
        client: &reqwest::Client,
        request: &HttpRequest,
    ) -> Result<HttpResponse, DiscoveryError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|err| DiscoveryError::Http(format!("invalid HTTP method: {err}")))?;

        let mut builder = client.request(method, &request.url);

        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }

        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }

        let response = builder.send().await.map_err(map_reqwest_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(map_reqwest_error)?.to_vec();

        Ok(HttpResponse { status, body })
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, DiscoveryError> {
        // This is a single deadline covering DNS (including fallback
        // servers), connection attempts, TLS and body reading.
        match tokio::time::timeout(self.timeout, self.send_once(&self.client, &request)).await {
            Ok(result) => result,
            Err(_) => Err(DiscoveryError::Timeout),
        }
    }
}

fn map_reqwest_error(err: reqwest::Error) -> DiscoveryError {
    if err.is_timeout() {
        DiscoveryError::Timeout
    } else {
        DiscoveryError::Http(err.to_string())
    }
}

/// HTTP transport errors.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),

    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("HTTP request timed out")]
    Timeout,

    #[error(transparent)]
    Dns(#[from] DnsError),
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
