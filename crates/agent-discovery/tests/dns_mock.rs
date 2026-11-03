//! DNS tests against local mock UDP servers.
//!
//! Every test binds `127.0.0.1:0`, so no external DNS or network access is
//! required and no real resolver configuration is changed.

use std::net::SocketAddr;
use std::time::Duration;

use agent_discovery::dns::{DnsError, DnsResolver, PreferIp, ResolverConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

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

fn response(query: &[u8], flags: u16, answers: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let id = [query[0], query[1]];
    let question = &query[12..];
    let mut packet = Vec::new();

    packet.extend_from_slice(&id);
    packet.extend_from_slice(&flags.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    packet.extend_from_slice(&(answers.len() as u16).to_be_bytes()); // ANCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(question);

    for (record_type, data) in answers {
        packet.extend_from_slice(&[0xC0, 0x0C]); // pointer to question name
        packet.extend_from_slice(&record_type.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&60u32.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(data);
    }

    packet
}

type Handler = Box<dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync>;

async fn start_dns(handler: Handler) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();

    tokio::spawn(async move {
        let mut buffer = vec![0u8; 2048];
        loop {
            let Ok((length, peer)) = socket.recv_from(&mut buffer).await else {
                break;
            };
            if let Some(answer) = handler(&buffer[..length]) {
                let _ = socket.send_to(&answer, peer).await;
            }
        }
    });

    address
}

fn custom_resolver(server: SocketAddr, prefer: PreferIp) -> DnsResolver {
    DnsResolver::new(ResolverConfig {
        custom_dns: Some(server.to_string()),
        timeout: Duration::from_millis(500),
        fallback_servers: Vec::new(),
        system_fallback: false,
        prefer_ip: prefer,
    })
    .unwrap()
}

async fn start_truncated_dns() -> SocketAddr {
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = udp.local_addr().unwrap();
    let tcp = TcpListener::bind(address).await.unwrap();

    tokio::spawn(async move {
        let mut buffer = vec![0u8; 2048];
        loop {
            let Ok((length, peer)) = udp.recv_from(&mut buffer).await else {
                break;
            };
            let query = &buffer[..length];
            let mut answer = response(query, 0x8380, &[(1, vec![203, 0, 113, 9])]);
            // Simulate a UDP packet cut in the middle of the A record data.
            answer.truncate(answer.len() - 2);
            let _ = udp.send_to(&answer, peer).await;
        }
    });

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = tcp.accept().await else {
                break;
            };

            let Ok(query_length) = stream.read_u16().await else {
                continue;
            };
            let mut query = vec![0u8; query_length as usize];
            if stream.read_exact(&mut query).await.is_err() {
                continue;
            }

            let answer = match question_type(&query) {
                Some(1) => response(&query, 0x8180, &[(1, vec![203, 0, 113, 9])]),
                _ => response(&query, 0x8180, &[]),
            };
            let _ = stream.write_u16(answer.len() as u16).await;
            let _ = stream.write_all(&answer).await;
            let _ = stream.flush().await;
        }
    });

    address
}

#[tokio::test]
async fn retries_truncated_udp_response_over_tcp() {
    let server = start_truncated_dns().await;
    let resolver = custom_resolver(server, PreferIp::V4);

    let addresses = resolver.resolve_host("example.com").await.unwrap();
    assert_eq!(
        addresses,
        vec!["203.0.113.9".parse::<std::net::IpAddr>().unwrap()]
    );
}

#[tokio::test]
async fn classifies_refused() {
    let server = start_dns(Box::new(|query| Some(response(query, 0x8185, &[])))).await;
    let resolver = custom_resolver(server, PreferIp::Auto);

    let error = resolver.resolve_host("example.com").await.unwrap_err();
    assert!(matches!(error, DnsError::Refused), "{error:?}");
}

#[tokio::test]
async fn resolves_a_and_aaaa_from_custom_server_and_orders_ipv4_first() {
    let server = start_dns(Box::new(|query| match question_type(query)? {
        1 => Some(response(
            query,
            0x8180,
            &[(1, vec![203, 0, 113, 9]), (1, vec![203, 0, 113, 7])],
        )),
        28 => Some(response(
            query,
            0x8180,
            &[(
                28,
                "2001:db8::1"
                    .parse::<std::net::Ipv6Addr>()
                    .unwrap()
                    .octets()
                    .to_vec(),
            )],
        )),
        _ => None,
    }))
    .await;

    let resolver = custom_resolver(server, PreferIp::V4);
    let addresses = resolver.resolve_host("example.com").await.unwrap();
    let rendered: Vec<String> = addresses.iter().map(ToString::to_string).collect();

    assert_eq!(rendered, vec!["203.0.113.9", "203.0.113.7", "2001:db8::1"]);
}

#[tokio::test]
async fn prefers_ipv6_when_configured() {
    let server = start_dns(Box::new(|query| match question_type(query)? {
        1 => Some(response(query, 0x8180, &[(1, vec![198, 51, 100, 1])])),
        28 => Some(response(
            query,
            0x8180,
            &[(
                28,
                "2001:db8::2"
                    .parse::<std::net::Ipv6Addr>()
                    .unwrap()
                    .octets()
                    .to_vec(),
            )],
        )),
        _ => None,
    }))
    .await;

    let resolver = custom_resolver(server, PreferIp::V6);
    let addresses = resolver.resolve_host("example.com").await.unwrap();

    assert!(addresses[0].is_ipv6());
    assert!(addresses[1].is_ipv4());
}

#[tokio::test]
async fn classifies_nxdomain() {
    let server = start_dns(Box::new(|query| Some(response(query, 0x8183, &[])))).await;
    let resolver = custom_resolver(server, PreferIp::Auto);

    let error = resolver.resolve_host("missing.example").await.unwrap_err();
    assert!(matches!(error, DnsError::NxDomain), "{error:?}");
}

#[tokio::test]
async fn classifies_timeout_when_server_never_answers() {
    // Bind but never read, so the UDP packet is accepted and no reply is sent.
    let blackhole = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = DnsResolver::new(ResolverConfig {
        custom_dns: Some(blackhole.local_addr().unwrap().to_string()),
        timeout: Duration::from_millis(120),
        fallback_servers: Vec::new(),
        system_fallback: false,
        prefer_ip: PreferIp::Auto,
    })
    .unwrap();

    let error = resolver.resolve_host("example.com").await.unwrap_err();
    assert!(matches!(error, DnsError::Timeout), "{error:?}");
}

#[tokio::test]
async fn classifies_truncated_udp_response() {
    let server = start_dns(Box::new(|query| Some(response(query, 0x8380, &[])))).await;
    let resolver = custom_resolver(server, PreferIp::Auto);

    let error = resolver.resolve_host("example.com").await.unwrap_err();
    assert!(matches!(error, DnsError::Truncated), "{error:?}");
}

#[tokio::test]
async fn falls_back_to_next_dns_server_on_servfail() {
    let primary = start_dns(Box::new(|query| Some(response(query, 0x8182, &[])))).await;
    let secondary = start_dns(Box::new(|query| match question_type(query)? {
        1 => Some(response(query, 0x8180, &[(1, vec![192, 0, 2, 10])])),
        _ => Some(response(query, 0x8180, &[])),
    }))
    .await;

    let resolver = DnsResolver::new(ResolverConfig {
        custom_dns: Some(primary.to_string()),
        timeout: Duration::from_millis(500),
        fallback_servers: vec![secondary.to_string()],
        system_fallback: false,
        prefer_ip: PreferIp::V4,
    })
    .unwrap();

    let addresses = resolver.resolve_host("example.com").await.unwrap();
    assert_eq!(
        addresses,
        vec!["192.0.2.10".parse::<std::net::IpAddr>().unwrap()]
    );
}

#[tokio::test]
async fn falls_back_to_system_resolver() {
    let blackhole = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = DnsResolver::new(ResolverConfig {
        custom_dns: Some(blackhole.local_addr().unwrap().to_string()),
        timeout: Duration::from_millis(100),
        fallback_servers: Vec::new(),
        system_fallback: true,
        prefer_ip: PreferIp::Auto,
    })
    .unwrap();

    let addresses = resolver.resolve_host("localhost").await.unwrap();
    assert!(addresses.iter().any(|address| address.is_loopback()));
}

#[tokio::test]
async fn ip_literals_do_not_use_dns() {
    let blackhole = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = DnsResolver::new(ResolverConfig {
        custom_dns: Some(blackhole.local_addr().unwrap().to_string()),
        timeout: Duration::from_millis(100),
        fallback_servers: Vec::new(),
        system_fallback: false,
        prefer_ip: PreferIp::Auto,
    })
    .unwrap();

    let addresses = resolver.resolve_host("127.0.0.1").await.unwrap();
    assert_eq!(
        addresses,
        vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()]
    );
}
