//! WebSocket transport abstraction and the production `tokio-tungstenite`
//! implementation.
//!
//! The agent only needs to write reports and heartbeats and to drain whatever
//! the server sends. Splitting sends and receives into two traits keeps the
//! reconnect loop free of `&mut` aliasing problems and lets tests substitute a
//! channel-backed transport.
//!
//! TCP connections are established by [`DnsResolver`] before the WebSocket
//! handshake, so `custom_dns` and `prefer_ip_version` also apply to `wss://`
//! endpoints. TLS still verifies the certificate against the hostname from the
//! URL, never against the resolved IP.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use agent_discovery::DnsResolver;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{Connector, WebSocketStream, client_async_tls_with_config};
use url::Url;

/// Connection/handshake failures.
#[derive(Debug, Error)]
pub enum WsError {
    #[error("invalid WebSocket URL {url:?}: {source}")]
    InvalidUrl {
        url: String,
        #[source]
        source: url::ParseError,
    },

    #[error("WebSocket URL has no host: {0}")]
    MissingHost(String),

    #[error("invalid request header {name}: {value}")]
    InvalidHeader { name: String, value: String },

    #[error("WebSocket handshake failed: {0}")]
    Handshake(String),

    #[error("WebSocket transport I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("WebSocket send failed: {0}")]
    Send(String),

    #[error("WebSocket read failed: {0}")]
    Read(String),

    #[error("no addresses found for WebSocket endpoint {0}")]
    NoAddresses(String),

    #[error("WebSocket handshake timed out")]
    Timeout,
}

/// An event delivered by the receiving half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsEvent {
    /// A text or binary frame. The agent ignores these (the panel must not send
    /// commands), but surfacing them keeps the drain observable in tests.
    Message(Vec<u8>),
    /// The peer closed the connection.
    Closed,
    /// Ping/pong control frames. `tungstenite` answers pings automatically.
    Control,
}

/// Writing half of a WebSocket connection.
#[async_trait]
pub trait WsSender: Send {
    async fn send_text(&mut self, payload: &str) -> Result<(), WsError>;
    async fn send_ping(&mut self) -> Result<(), WsError>;
    async fn close(&mut self);
}

/// Reading half of a WebSocket connection.
#[async_trait]
pub trait WsReceiver: Send {
    /// Next event, or `None` when the stream is exhausted.
    async fn next_event(&mut self) -> Option<Result<WsEvent, WsError>>;
}

/// A connected WebSocket split into independent halves.
pub struct WsConnection {
    pub sender: Box<dyn WsSender>,
    pub receiver: Box<dyn WsReceiver>,
}

/// Injectable connection factory.
#[async_trait]
pub trait WsTransport: Send + Sync {
    async fn connect(&self, endpoint: &str) -> Result<WsConnection, WsError>;
}

/// TLS and handshake policy for the production transport.
#[derive(Clone)]
pub struct WsClientConfig {
    pub resolver: DnsResolver,
    pub handshake_timeout: Duration,
    pub ignore_unsafe_cert: bool,
    pub cf_access_client_id: String,
    pub cf_access_client_secret: String,
}

impl WsClientConfig {
    pub fn new(resolver: DnsResolver) -> Self {
        Self {
            resolver,
            handshake_timeout: Duration::from_secs(15),
            ignore_unsafe_cert: false,
            cf_access_client_id: String::new(),
            cf_access_client_secret: String::new(),
        }
    }

    fn access_headers(&self) -> Vec<(&'static str, HeaderValue)> {
        if self.cf_access_client_id.is_empty() || self.cf_access_client_secret.is_empty() {
            return Vec::new();
        }
        vec![
            (
                "CF-Access-Client-Id",
                HeaderValue::from_str(&self.cf_access_client_id)
                    .unwrap_or(HeaderValue::from_static("")),
            ),
            (
                "CF-Access-Client-Secret",
                HeaderValue::from_str(&self.cf_access_client_secret)
                    .unwrap_or(HeaderValue::from_static("")),
            ),
        ]
    }
}

/// Production transport.
pub struct TungsteniteTransport {
    config: WsClientConfig,
}

impl TungsteniteTransport {
    pub fn new(config: WsClientConfig) -> Self {
        Self { config }
    }

    async fn connect_one(&self, url: &str, address: SocketAddr) -> Result<WsConnection, WsError> {
        let stream =
            tokio::time::timeout(self.config.handshake_timeout, TcpStream::connect(address))
                .await
                .map_err(|_| WsError::Timeout)??;
        stream.set_nodelay(true)?;

        let mut request = url
            .into_client_request()
            .map_err(|err| WsError::Handshake(err.to_string()))?;
        for (name, value) in self.config.access_headers() {
            request.headers_mut().insert(name, value);
        }

        let connector = if url.starts_with("wss://") {
            if self.config.ignore_unsafe_cert {
                Some(Connector::Rustls(Arc::new(insecure_rustls_config()?)))
            } else {
                None
            }
        } else {
            Some(Connector::Plain)
        };

        let handshake = tokio::time::timeout(
            self.config.handshake_timeout,
            client_async_tls_with_config(request, stream, None, connector),
        )
        .await
        .map_err(|_| WsError::Timeout)?;

        let (websocket, _response) =
            handshake.map_err(|err| WsError::Handshake(err.to_string()))?;
        Ok(split(websocket))
    }
}

#[async_trait]
impl WsTransport for TungsteniteTransport {
    async fn connect(&self, endpoint: &str) -> Result<WsConnection, WsError> {
        let url = Url::parse(endpoint).map_err(|source| WsError::InvalidUrl {
            url: endpoint.to_owned(),
            source,
        })?;

        let host = url
            .host_str()
            .ok_or_else(|| WsError::MissingHost(endpoint.to_owned()))?;
        let port = url.port_or_known_default().unwrap_or(80);

        let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => self
                .config
                .resolver
                .resolve_host(host)
                .await
                .map_err(|err| WsError::NoAddresses(format!("{host}: {err}")))?,
        };

        if addresses.is_empty() {
            return Err(WsError::NoAddresses(host.to_owned()));
        }

        let mut last_error = None;
        for ip in addresses {
            match self.connect_one(endpoint, SocketAddr::new(ip, port)).await {
                Ok(connection) => return Ok(connection),
                Err(err) => {
                    tracing::debug!(%ip, %err, "WebSocket connection attempt failed");
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| WsError::NoAddresses(host.to_owned())))
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn split(websocket: Socket) -> WsConnection {
    let (sink, stream) = websocket.split();
    WsConnection {
        sender: Box::new(TungsteniteSender { sink }),
        receiver: Box::new(TungsteniteReceiver { stream }),
    }
}

struct TungsteniteSender {
    sink: futures_util::stream::SplitSink<Socket, Message>,
}

#[async_trait]
impl WsSender for TungsteniteSender {
    async fn send_text(&mut self, payload: &str) -> Result<(), WsError> {
        self.sink
            .send(Message::Text(payload.to_owned().into()))
            .await
            .map_err(|err| WsError::Send(err.to_string()))
    }

    async fn send_ping(&mut self) -> Result<(), WsError> {
        self.sink
            .send(Message::Ping(Default::default()))
            .await
            .map_err(|err| WsError::Send(err.to_string()))
    }

    async fn close(&mut self) {
        let _ = self.sink.close().await;
    }
}

struct TungsteniteReceiver {
    stream: futures_util::stream::SplitStream<Socket>,
}

#[async_trait]
impl WsReceiver for TungsteniteReceiver {
    async fn next_event(&mut self) -> Option<Result<WsEvent, WsError>> {
        match self.stream.next().await? {
            Ok(Message::Text(text)) => Some(Ok(WsEvent::Message(text.as_bytes().to_vec()))),
            Ok(Message::Binary(bytes)) => Some(Ok(WsEvent::Message(bytes.to_vec()))),
            Ok(Message::Close(_)) => Some(Ok(WsEvent::Closed)),
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                Some(Ok(WsEvent::Control))
            }
            Err(err) => Some(Err(WsError::Read(err.to_string()))),
        }
    }
}

/// rustls configuration that accepts any certificate.
///
/// Only used when `ignore_unsafe_cert` is enabled, matching Go's
/// `InsecureSkipVerify`. The hostname is still sent as SNI.
fn insecure_rustls_config() -> Result<rustls::ClientConfig, WsError> {
    let verifier = Arc::new(NoCertificateVerification);
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(config)
}

#[derive(Debug)]
struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::CryptoProvider::get_default()
            .map(|provider| {
                provider
                    .signature_verification_algorithms
                    .supported_schemes()
            })
            .unwrap_or_else(|| {
                rustls::crypto::aws_lc_rs::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            })
    }
}
