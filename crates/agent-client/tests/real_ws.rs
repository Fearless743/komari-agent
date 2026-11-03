//! Real WebSocket transport round-trip against a local mock server.
//!
//! This complements the channel-based loop tests: it exercises the actual
//! `tokio-tungstenite` handshake, DNS bypass for an IP literal, text frames and
//! close handling. The server is local and the test never touches a panel.

use std::time::Duration;

use agent_client::{TungsteniteTransport, WsClientConfig, WsEvent, WsTransport};
use agent_discovery::{DnsResolver, PreferIp, ResolverConfig};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::{TcpListener, UdpSocket};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn real_transport_connects_sends_text_and_drains_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_async(stream).await.unwrap();

        let message = websocket.next().await.unwrap().unwrap();
        assert_eq!(message, Message::Text("{\"cpu\":{}}".into()));

        // Control frames must not interrupt the drain.
        websocket
            .send(Message::Ping(Vec::new().into()))
            .await
            .unwrap();
        websocket.send(Message::Close(None)).await.unwrap();
    });

    let transport = TungsteniteTransport::new(WsClientConfig::new(DnsResolver::system_only()));
    let mut connection = transport
        .connect(&format!(
            "ws://127.0.0.1:{}/api/clients/report?token=t",
            address.port()
        ))
        .await
        .unwrap();

    connection.sender.send_text("{\"cpu\":{}}").await.unwrap();

    let mut closed = false;
    for _ in 0..8 {
        let event = tokio::time::timeout(Duration::from_secs(2), connection.receiver.next_event())
            .await
            .expect("timed out waiting for a server frame")
            .expect("receiver ended without a close event")
            .unwrap();

        match event {
            WsEvent::Closed => {
                closed = true;
                break;
            }
            WsEvent::Control | WsEvent::Message(_) => {}
        }
    }

    assert!(closed, "close frame was not surfaced");
    server.await.unwrap();
}

fn question_type(query: &[u8]) -> Option<u16> {
    let mut position = 12;
    loop {
        let length = *query.get(position)? as usize;
        if length == 0 {
            position += 1;
            break;
        }
        position += 1 + length;
    }
    Some(u16::from_be_bytes([
        *query.get(position)?,
        *query.get(position + 1)?,
    ]))
}

fn dns_response(query: &[u8], record_type: u16, data: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.extend_from_slice(&query[..2]);
    packet.extend_from_slice(&0x8180u16.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&query[12..]);
    packet.extend_from_slice(&[0xC0, 0x0C]);
    packet.extend_from_slice(&record_type.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&60u32.to_be_bytes());
    packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
    packet.extend_from_slice(data);
    packet
}

async fn start_dns() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 1024];
        loop {
            let Ok((length, peer)) = socket.recv_from(&mut buffer).await else {
                break;
            };
            let query = &buffer[..length];
            let answer = match question_type(query) {
                Some(1) => dns_response(query, 1, &[127, 0, 0, 1]),
                // No AAAA record: the resolver still returns the IPv4 answer.
                _ => dns_response(query, 28, &[0; 0]),
            };
            let _ = socket.send_to(&answer, peer).await;
        }
    });
    address
}

#[tokio::test]
async fn custom_dns_resolves_the_websocket_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_address = listener.local_addr().unwrap();
    let dns_address = start_dns().await;

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_async(stream).await.unwrap();
        let message = websocket.next().await.unwrap().unwrap();
        assert_eq!(message, Message::Text("hello".into()));
        websocket.close(None).await.unwrap();
    });

    let resolver = DnsResolver::new(ResolverConfig {
        custom_dns: Some(dns_address.to_string()),
        timeout: Duration::from_secs(2),
        fallback_servers: Vec::new(),
        system_fallback: false,
        prefer_ip: PreferIp::V4,
    })
    .unwrap();

    let transport = TungsteniteTransport::new(WsClientConfig::new(resolver));
    let mut connection = transport
        .connect(&format!(
            "ws://panel.test:{}/api/clients/report?token=t",
            ws_address.port()
        ))
        .await
        .unwrap();

    connection.sender.send_text("hello").await.unwrap();
    server.await.unwrap();
}
