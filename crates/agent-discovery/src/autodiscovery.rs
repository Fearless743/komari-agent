//! Auto discovery registration, mirroring `komari-agent/cmd/autodiscovery.go`.
//!
//! The flow is:
//!
//! 1. If `auto-discovery.json` exists next to the executable, reuse its token.
//! 2. Otherwise POST the discovery key to
//!    `{endpoint}/api/clients/register?name={hostname}`.
//! 3. Validate the response, persist `{uuid, token}` to `auto-discovery.json`
//!    and return the credentials to the caller.
//!
//! The HTTP client and the configuration path are injectable so tests never
//! touch the network or the real executable directory.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::dns::{DnsError, DnsResolver, PreferIp, ResolverConfig};
use crate::idna::convert_url_to_ascii;

/// File name used by the Go agent and this implementation.
pub const AUTO_DISCOVERY_FILE: &str = "auto-discovery.json";

/// Credentials persisted after a successful registration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoDiscoveryConfig {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub token: String,
}

/// Result of [`AutoDiscovery::handle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryResult {
    /// Panel endpoint that should be used with these credentials.
    pub endpoint: String,
    pub config: AutoDiscoveryConfig,
    /// `true` when a new registration was performed, `false` when an existing
    /// file was reused.
    pub registered: bool,
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
/// attempts inside the connector. This avoids re-sending a POST after it may
/// already have reached the server.
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

        let client =
            crate::http_client::build_reqwest_client(resolver, timeout, ignore_unsafe_cert)
                .map_err(|err| {
                    DiscoveryError::Http(format!("failed to build HTTP client: {err}"))
                })?;

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

/// Configuration for [`AutoDiscovery`].
#[derive(Clone)]
pub struct AutoDiscoveryOptions {
    pub endpoint: String,
    pub auto_discovery_key: String,
    /// Hostname sent as the `name` query parameter. Defaults to the system
    /// hostname.
    pub hostname: Option<String>,
    /// Path of `auto-discovery.json`. Defaults to the executable directory.
    pub config_path: Option<PathBuf>,
    pub cf_access_client_id: String,
    pub cf_access_client_secret: String,
    pub timeout: Duration,
    pub ignore_unsafe_cert: bool,
    pub prefer_ip: PreferIp,
    /// Custom DNS server; used when `resolver` is not supplied.
    pub custom_dns: Option<String>,
    /// Explicit resolver, normally shared with the agent's HTTP client.
    pub resolver: Option<DnsResolver>,
    /// Explicit HTTP transport, used by tests.
    pub http: Option<Arc<dyn HttpClient>>,
}

impl Default for AutoDiscoveryOptions {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            auto_discovery_key: String::new(),
            hostname: None,
            config_path: None,
            cf_access_client_id: String::new(),
            cf_access_client_secret: String::new(),
            timeout: Duration::from_secs(30),
            ignore_unsafe_cert: false,
            prefer_ip: PreferIp::Auto,
            custom_dns: None,
            resolver: None,
            http: None,
        }
    }
}

/// Auto discovery client.
#[derive(Clone)]
pub struct AutoDiscovery {
    options: AutoDiscoveryOptions,
    resolver: DnsResolver,
    http: Arc<dyn HttpClient>,
    config_path: PathBuf,
}

impl AutoDiscovery {
    /// Build a client, validating the endpoint and resolving the transport.
    pub fn new(options: AutoDiscoveryOptions) -> Result<Self, DiscoveryError> {
        let endpoint = options.endpoint.trim().to_owned();
        if endpoint.is_empty() {
            return Err(DiscoveryError::InvalidEndpoint(
                "endpoint is empty".to_owned(),
            ));
        }

        let ascii_endpoint = convert_url_to_ascii(&endpoint).unwrap_or_else(|err| {
            tracing::warn!(%err, "failed to convert endpoint IDN to ASCII; using original value");
            endpoint.clone()
        });
        let parsed = Url::parse(&ascii_endpoint)
            .map_err(|err| DiscoveryError::InvalidEndpoint(err.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(DiscoveryError::InvalidEndpoint(format!(
                "unsupported URL scheme {}",
                parsed.scheme()
            )));
        }

        let resolver = match &options.resolver {
            Some(resolver) => resolver.clone(),
            None => DnsResolver::new(ResolverConfig {
                custom_dns: options.custom_dns.clone(),
                prefer_ip: options.prefer_ip,
                ..ResolverConfig::default()
            })?,
        };

        let timeout = if options.timeout.is_zero() {
            Duration::from_secs(30)
        } else {
            options.timeout
        };

        let http = match &options.http {
            Some(http) => http.clone(),
            None => Arc::new(ReqwestHttpClient::new(
                resolver.clone(),
                timeout,
                options.ignore_unsafe_cert,
            )?),
        };

        let config_path = options
            .config_path
            .clone()
            .unwrap_or_else(default_config_path);

        let mut options = options;
        options.endpoint = endpoint;

        Ok(Self {
            options,
            resolver,
            http,
            config_path,
        })
    }

    /// Effective resolver.
    pub fn resolver(&self) -> &DnsResolver {
        &self.resolver
    }

    /// Panel endpoint used for registration and by the caller afterwards.
    pub fn endpoint(&self) -> &str {
        &self.options.endpoint
    }

    /// Path used for `auto-discovery.json`.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Load the persisted credentials.
    ///
    /// A missing file returns `Ok(None)`, matching the Go implementation.
    pub fn load_config(&self) -> Result<Option<AutoDiscoveryConfig>, DiscoveryError> {
        let bytes = match fs::read(&self.config_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(DiscoveryError::Io {
                    path: self.config_path.clone(),
                    source,
                });
            }
        };

        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| DiscoveryError::InvalidConfig {
                path: self.config_path.clone(),
                source,
            })
    }

    /// Persist credentials using the same pretty-printed JSON as Go.
    pub fn save_config(&self, config: &AutoDiscoveryConfig) -> Result<(), DiscoveryError> {
        if let Some(parent) = self.config_path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            fs::create_dir_all(parent).map_err(|source| DiscoveryError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let bytes = serde_json::to_vec_pretty(config).map_err(DiscoveryError::Serialize)?;
        fs::write(&self.config_path, bytes).map_err(|source| DiscoveryError::Io {
            path: self.config_path.clone(),
            source,
        })
    }

    /// Reuse an existing token or register a new client.
    pub async fn handle(&self) -> Result<DiscoveryResult, DiscoveryError> {
        match self.load_config() {
            Ok(Some(config)) => {
                tracing::info!(uuid = %config.uuid, "using existing auto-discovery token");
                Ok(DiscoveryResult {
                    endpoint: self.options.endpoint.clone(),
                    config,
                    registered: false,
                })
            }
            Ok(None) => {
                tracing::info!("auto-discovery config not found, registering with server");
                let config = self.register().await?;
                Ok(DiscoveryResult {
                    endpoint: self.options.endpoint.clone(),
                    config,
                    registered: true,
                })
            }
            Err(err) => {
                // The Go agent logs the load error and continues with
                // registration. A corrupt file is therefore replaced on a
                // successful registration.
                tracing::warn!(%err, "failed to load auto-discovery config; registering again");
                let config = self.register().await?;
                Ok(DiscoveryResult {
                    endpoint: self.options.endpoint.clone(),
                    config,
                    registered: true,
                })
            }
        }
    }

    /// Perform the registration request and persist the result.
    pub async fn register(&self) -> Result<AutoDiscoveryConfig, DiscoveryError> {
        let hostname = self
            .options
            .hostname
            .clone()
            .or_else(|| {
                hostname::get()
                    .ok()
                    .and_then(|name| name.into_string().ok())
            })
            .unwrap_or_default();

        let endpoint = convert_url_to_ascii(&self.options.endpoint).unwrap_or_else(|err| {
            tracing::warn!(%err, "failed to convert endpoint IDN to ASCII; using original value");
            self.options.endpoint.clone()
        });

        let mut url = Url::parse(&endpoint)
            .map_err(|err| DiscoveryError::InvalidEndpoint(err.to_string()))?;
        let path = format!("{}/api/clients/register", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut().append_pair("name", &hostname);

        let body = serde_json::to_vec(&RegisterRequest {
            key: self.options.auto_discovery_key.clone(),
        })
        .map_err(DiscoveryError::Serialize)?;

        let mut headers = vec![
            ("Content-Type".to_owned(), "application/json".to_owned()),
            (
                "Authorization".to_owned(),
                format!("Bearer {}", self.options.auto_discovery_key),
            ),
        ];

        if !self.options.cf_access_client_id.is_empty()
            && !self.options.cf_access_client_secret.is_empty()
        {
            headers.push((
                "CF-Access-Client-Id".to_owned(),
                self.options.cf_access_client_id.clone(),
            ));
            headers.push((
                "CF-Access-Client-Secret".to_owned(),
                self.options.cf_access_client_secret.clone(),
            ));
        }

        let response = self
            .http
            .execute(HttpRequest {
                method: "POST".to_owned(),
                url: url.to_string(),
                headers,
                body: Some(body),
            })
            .await?;

        if response.status != 200 {
            return Err(DiscoveryError::HttpStatus {
                status: response.status,
                body: String::from_utf8_lossy(&response.body).into_owned(),
            });
        }

        let register: RegisterResponse =
            serde_json::from_slice(&response.body).map_err(DiscoveryError::InvalidJson)?;

        if register.status != "success" {
            return Err(DiscoveryError::Rejected(register.message));
        }

        let config = AutoDiscoveryConfig {
            uuid: register.data.uuid,
            token: register.data.token,
        };

        self.save_config(&config)?;
        tracing::info!(uuid = %config.uuid, "registered with auto-discovery");

        Ok(config)
    }
}

/// `auto-discovery.json` next to the executable, or the current directory if
/// the executable path is unavailable.
pub fn default_config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(AUTO_DISCOVERY_FILE)))
        .unwrap_or_else(|| PathBuf::from(AUTO_DISCOVERY_FILE))
}

#[derive(Serialize)]
struct RegisterRequest {
    key: String,
}

#[derive(Deserialize)]
struct RegisterResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: RegisterData,
}

#[derive(Default, Deserialize)]
struct RegisterData {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    token: String,
}

/// Auto discovery errors.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),

    #[error("failed to read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse auto-discovery config {path}: {source}")]
    InvalidConfig {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to parse register response: {0}")]
    InvalidJson(#[source] serde_json::Error),

    #[error("failed to serialize request: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error("register request failed with status {status}: {body}")]
    HttpStatus { status: u16, body: String },

    #[error("register request failed: {0}")]
    Rejected(String),

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

    #[test]
    fn default_config_file_is_auto_discovery_json() {
        assert!(default_config_path().ends_with(AUTO_DISCOVERY_FILE));
    }

    #[test]
    fn rejects_empty_endpoint() {
        let error = AutoDiscovery::new(AutoDiscoveryOptions::default())
            .err()
            .expect("empty endpoint must fail");
        assert!(matches!(error, DiscoveryError::InvalidEndpoint(_)));
    }

    #[test]
    fn rejects_non_http_schemes() {
        let error = AutoDiscovery::new(AutoDiscoveryOptions {
            endpoint: "ftp://example.com".to_owned(),
            ..AutoDiscoveryOptions::default()
        })
        .err()
        .expect("non-http scheme must fail");

        assert!(matches!(error, DiscoveryError::InvalidEndpoint(_)));
    }
}
