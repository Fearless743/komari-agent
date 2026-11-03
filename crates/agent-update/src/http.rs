//! Injectable HTTP transport for update checks and downloads.
//!
//! The production implementation uses `reqwest`; tests supply their own
//! implementation so no real network requests or binary replacement occur.

use async_trait::async_trait;
use std::time::Duration;

use crate::error::UpdateError;

/// A simple HTTP GET request.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// A complete HTTP response body.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Minimal HTTP client abstraction used by the updater.
#[async_trait]
pub trait UpdateHttp: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, UpdateError>;
}

/// Production `reqwest` implementation.
#[derive(Clone, Debug)]
pub struct ReqwestUpdateHttp {
    client: reqwest::Client,
}

impl ReqwestUpdateHttp {
    /// Build a client with the given timeout and certificate policy.
    pub fn new(timeout: Duration, ignore_unsafe_cert: bool) -> Self {
        let timeout = if timeout.is_zero() {
            Duration::from_secs(60)
        } else {
            timeout
        };

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .danger_accept_invalid_certs(ignore_unsafe_cert)
            .user_agent(format!("komari-agent-rs/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Use an externally configured client.
    ///
    /// This is how the agent injects the custom DNS transport built by
    /// `agent-discovery`; the update crate stays independent of it.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl UpdateHttp for ReqwestUpdateHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, UpdateError> {
        let mut builder = self.client.get(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }

        let response = builder
            .send()
            .await
            .map_err(|err| UpdateError::Http(err.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|err| UpdateError::Http(err.to_string()))?
            .to_vec();

        Ok(HttpResponse { status, body })
    }
}
