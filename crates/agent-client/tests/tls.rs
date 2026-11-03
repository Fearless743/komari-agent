//! TLS policy tests for the production WebSocket transport.
//!
//! The fixtures are a self-signed certificate for `127.0.0.1`; no external CA
//! or network is involved.

use std::time::Duration;

use agent_client::{TungsteniteTransport, WsClientConfig, WsTransport};
use agent_discovery::DnsResolver;
use futures_util::StreamExt;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

fn cert() -> CertificateDer<'static> {
    CertificateDer::from(include_bytes!("fixtures/test-cert.der").to_vec())
}

fn key() -> PrivateKeyDer<'static> {
    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        include_bytes!("fixtures/test-key.der").to_vec(),
    ))
}

async fn start_tls_server() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert()], key())
        .unwrap();
    let acceptor = TlsAcceptor::from(std::sync::Arc::new(config));

    let handle = tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(tls) = acceptor.accept(stream).await else {
            // The default-verification test is expected to fail here.
            return;
        };
        let mut websocket = accept_async(tls).await.unwrap();
        if let Some(Ok(Message::Text(text))) = websocket.next().await {
            assert_eq!(text, "tls");
        }
        let _ = websocket.close(None).await;
    });

    (port, handle)
}

fn transport(ignore_unsafe_cert: bool) -> TungsteniteTransport {
    let mut config = WsClientConfig::new(DnsResolver::system_only());
    config.ignore_unsafe_cert = ignore_unsafe_cert;
    config.handshake_timeout = Duration::from_secs(3);
    TungsteniteTransport::new(config)
}

#[tokio::test]
async fn ignore_unsafe_cert_accepts_a_self_signed_server() {
    let (port, server) = start_tls_server().await;
    let mut connection = tokio::time::timeout(
        Duration::from_secs(5),
        transport(true).connect(&format!(
            "wss://127.0.0.1:{port}/api/clients/report?token=t"
        )),
    )
    .await
    .expect("connect timed out")
    .expect("insecure TLS connection failed");

    connection.sender.send_text("tls").await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn default_verification_rejects_a_self_signed_server() {
    let (port, server) = start_tls_server().await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        transport(false).connect(&format!(
            "wss://127.0.0.1:{port}/api/clients/report?token=t"
        )),
    )
    .await
    .expect("connect timed out");

    assert!(result.is_err(), "self-signed certificate was accepted");
    let _ = server.await;
}
