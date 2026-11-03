//! Custom DNS resolution for the Komari agent.
//!
//! This module mirrors `komari-agent/dnsresolver/resolver.go`:
//!
//! * a custom DNS server (`custom_dns`, `host:port`) is queried first;
//! * the built-in server list is used as a fallback when the custom server is
//!   unreachable;
//! * without a custom server the operating system resolver is used;
//! * A and AAAA answers are merged and ordered according to the preferred IP
//!   version. `PreferIp::Auto` prefers IPv4 when a non-loopback IPv4 interface
//!   exists, exactly like the Go implementation.
//!
//! The wire protocol is implemented directly on top of UDP/TCP instead of
//! delegating to a resolver crate. That keeps the fallback and error
//! classification behaviour visible and testable, and avoids a large
//! dependency for the small subset of DNS the agent needs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

/// Built-in fallback servers, matching the Go `DNSServers` list.
pub const BUILTIN_DNS_SERVERS: &[&str] = &[
    "[2606:4700:4700::1111]:53",
    "[2606:4700:4700::1001]:53",
    "[2001:4860:4860::8888]:53",
    "[2001:4860:4860::8844]:53",
    "114.114.114.114:53",
    "1.1.1.1:53",
    "8.8.8.8:53",
    "8.8.4.4:53",
    "223.5.5.5:53",
    "119.29.29.29:53",
];

/// Default query timeout. The Go resolver uses a 10 second dial timeout and
/// relies on the standard library for the rest.
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Preferred IP version when connecting to the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreferIp {
    /// Prefer IPv4 when the host has a usable IPv4 interface, otherwise IPv6.
    #[default]
    Auto,
    V4,
    V6,
}

impl PreferIp {
    /// Parse the `prefer_ip_version` configuration value.
    pub fn parse(value: &str) -> Result<Self, DnsError> {
        match value.trim() {
            "" => Ok(Self::Auto),
            "4" => Ok(Self::V4),
            "6" => Ok(Self::V6),
            other => Err(DnsError::InvalidPreference(other.to_owned())),
        }
    }
}

/// DNS record types used by the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordType {
    A = 1,
    Aaaa = 28,
}

impl RecordType {
    fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Resolver configuration.
#[derive(Clone, Debug)]
pub struct ResolverConfig {
    /// Custom DNS server (`host:port`, IPv4/IPv6, with or without port).
    pub custom_dns: Option<String>,
    /// Per-query timeout.
    pub timeout: Duration,
    /// Fallback servers used when a custom server is configured.
    pub fallback_servers: Vec<String>,
    /// Use the operating system resolver as the final fallback.
    pub system_fallback: bool,
    /// Preferred IP version.
    pub prefer_ip: PreferIp,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            custom_dns: None,
            timeout: DEFAULT_QUERY_TIMEOUT,
            fallback_servers: BUILTIN_DNS_SERVERS
                .iter()
                .map(|server| (*server).to_owned())
                .collect(),
            system_fallback: true,
            prefer_ip: PreferIp::Auto,
        }
    }
}

impl ResolverConfig {
    /// Configuration for a custom DNS server with the Go fallback list.
    pub fn custom(server: impl Into<String>) -> Self {
        Self {
            custom_dns: Some(server.into()),
            ..Self::default()
        }
    }

    /// System-only resolution, matching Go when `custom_dns` is empty.
    pub fn system() -> Self {
        Self {
            custom_dns: None,
            fallback_servers: Vec::new(),
            system_fallback: true,
            ..Self::default()
        }
    }
}

/// DNS resolver with custom server, fallback list and system fallback.
#[derive(Clone, Debug)]
pub struct DnsResolver {
    config: ResolverConfig,
    custom: Option<String>,
}

impl DnsResolver {
    /// Create a resolver, normalizing the custom server address.
    pub fn new(mut config: ResolverConfig) -> Result<Self, DnsError> {
        let custom = match config.custom_dns.as_deref() {
            Some(server) if !server.trim().is_empty() => Some(normalize_dns_server(server)?),
            _ => None,
        };

        if config.timeout.is_zero() {
            config.timeout = DEFAULT_QUERY_TIMEOUT;
        }

        let mut fallback = Vec::with_capacity(config.fallback_servers.len());
        for server in &config.fallback_servers {
            if server.trim().is_empty() {
                continue;
            }
            fallback.push(normalize_dns_server(server)?);
        }
        config.fallback_servers = fallback;

        Ok(Self { config, custom })
    }

    /// Resolver that only uses the operating system resolver.
    pub fn system_only() -> Self {
        Self {
            custom: None,
            config: ResolverConfig::system(),
        }
    }

    /// Resolver using a custom server and the built-in fallback list.
    pub fn with_custom(server: impl Into<String>, prefer_ip: PreferIp) -> Result<Self, DnsError> {
        Self::new(ResolverConfig {
            custom_dns: Some(server.into()),
            prefer_ip,
            ..ResolverConfig::default()
        })
    }

    /// Effective configuration.
    pub fn config(&self) -> &ResolverConfig {
        &self.config
    }

    /// Normalized custom server, if configured.
    pub fn custom_server(&self) -> Option<&str> {
        self.custom.as_deref()
    }

    /// Resolve a host to IP addresses, ordered by the preferred IP version.
    ///
    /// IP literals are returned as-is. The result is de-duplicated while
    /// preserving the answer order within each address family.
    pub async fn resolve_host(&self, host: &str) -> Result<Vec<IpAddr>, DnsError> {
        let mut ips = self.resolve_host_unsorted(host).await?;
        sort_ips_by_preference(&mut ips, self.config.prefer_ip);
        Ok(ips)
    }

    /// Resolve a host without applying IP version ordering.
    pub async fn resolve_host_unsorted(&self, host: &str) -> Result<Vec<IpAddr>, DnsError> {
        let host = host.trim();
        let host = host.strip_suffix('.').unwrap_or(host);

        if host.is_empty() {
            return Err(DnsError::InvalidName(host.to_owned()));
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }

        let (a_result, aaaa_result) = tokio::join!(
            self.query_with_fallback(host, RecordType::A),
            self.query_with_fallback(host, RecordType::Aaaa)
        );

        let mut ips = Vec::new();
        let mut error = None;

        match a_result {
            Ok(mut answers) => ips.append(&mut answers),
            Err(err) => error = Some(err),
        }
        match aaaa_result {
            Ok(mut answers) => ips.append(&mut answers),
            Err(err) => error = Some(prefer_error(error.take(), err)),
        }

        dedupe(&mut ips);

        if ips.is_empty() {
            return Err(error.unwrap_or_else(|| DnsError::NoRecords(host.to_owned())));
        }

        Ok(ips)
    }

    /// Query a single record type, applying the custom/fallback policy.
    pub async fn query(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<Vec<IpAddr>, DnsError> {
        self.query_with_fallback(name, record_type).await
    }

    /// Connect to `host:port` using the resolver and preference order.
    pub async fn connect(&self, host: &str, port: u16) -> Result<TcpStream, DnsError> {
        let ips = self.resolve_host(host).await?;
        let mut last_error = None;

        for ip in ips {
            let address = SocketAddr::new(ip, port);
            match tokio::time::timeout(self.config.timeout, TcpStream::connect(address)).await {
                Ok(Ok(stream)) => return Ok(stream),
                Ok(Err(err)) => last_error = Some(err.to_string()),
                Err(_) => last_error = Some(format!("connect to {address} timed out")),
            }
        }

        Err(DnsError::AllServersFailed(
            last_error.unwrap_or_else(|| format!("no addresses for {host}")),
        ))
    }

    async fn query_with_fallback(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<Vec<IpAddr>, DnsError> {
        if self.custom.is_none() {
            return self.lookup_system(name, record_type).await;
        }

        let mut last_error = None;

        if let Some(server) = &self.custom {
            match self.query_server(server, name, record_type).await {
                Ok(ips) => return Ok(ips),
                Err(err) => {
                    if !should_fallback(&err) {
                        return Err(err);
                    }
                    tracing::debug!(server, %err, "custom DNS query failed");
                    last_error = Some(err);
                }
            }
        }

        for server in &self.config.fallback_servers {
            if self.custom.as_deref() == Some(server.as_str()) {
                continue;
            }
            match self.query_server(server, name, record_type).await {
                Ok(ips) => return Ok(ips),
                Err(err) => {
                    tracing::debug!(server, %err, "fallback DNS query failed");
                    last_error = Some(err);
                }
            }
        }

        if self.config.system_fallback {
            match self.lookup_system(name, record_type).await {
                Ok(ips) => return Ok(ips),
                Err(err) => {
                    last_error = Some(prefer_error(last_error.take(), err));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| DnsError::NoRecords(name.to_owned())))
    }

    async fn query_server(
        &self,
        server: &str,
        name: &str,
        record_type: RecordType,
    ) -> Result<Vec<IpAddr>, DnsError> {
        let address = resolve_server_address(server, self.config.timeout).await?;
        let query_name = to_ascii_name(name)?;
        let packet = build_query(next_id(), &query_name, record_type)?;

        let response =
            match tokio::time::timeout(self.config.timeout, query_udp(address, &packet)).await {
                Ok(result) => result?,
                Err(_) => return Err(DnsError::Timeout),
            };

        let parsed = parse_response(&response, query_id(&packet))?;

        if parsed.truncated {
            // A truncated UDP answer must be retried over TCP, like the Go
            // resolver does. If TCP is unavailable the truncation is surfaced.
            match tokio::time::timeout(self.config.timeout, query_tcp(address, &packet)).await {
                Ok(Ok(response)) => {
                    let parsed = parse_response(&response, query_id(&packet))?;
                    check_rcode(parsed.rcode)?;
                    return Ok(records_to_ips(parsed.answers));
                }
                Ok(Err(err)) => {
                    tracing::debug!(%err, "TCP DNS retry failed after truncation");
                    return Err(DnsError::Truncated);
                }
                Err(_) => return Err(DnsError::Truncated),
            }
        }

        check_rcode(parsed.rcode)?;
        Ok(records_to_ips(parsed.answers))
    }

    async fn lookup_system(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<Vec<IpAddr>, DnsError> {
        let lookup = tokio::net::lookup_host((name, 0));
        let addresses = match tokio::time::timeout(self.config.timeout, lookup).await {
            Ok(Ok(addresses)) => addresses,
            Ok(Err(err)) => return Err(DnsError::SystemLookup(name.to_owned(), err.to_string())),
            Err(_) => return Err(DnsError::Timeout),
        };

        let mut ips: Vec<IpAddr> = addresses
            .map(|address| address.ip())
            .filter(|ip| match record_type {
                RecordType::A => ip.is_ipv4(),
                RecordType::Aaaa => ip.is_ipv6(),
            })
            .collect();

        dedupe(&mut ips);

        if ips.is_empty() {
            return Err(DnsError::NoRecords(name.to_owned()));
        }

        Ok(ips)
    }
}

/// Normalize a DNS server string to `host:port`, matching the Go helper.
///
/// * bare IPv4/hostname gets `:53`;
/// * bare IPv6 gets brackets and `:53`;
/// * `[IPv6]:port` and `host:port` are kept as-is.
pub fn normalize_dns_server(server: &str) -> Result<String, DnsError> {
    let server = server.trim();
    if server.is_empty() {
        return Err(DnsError::InvalidServer(server.to_owned()));
    }

    if server.starts_with('[') {
        if server.contains("]:") {
            return Ok(server.to_owned());
        }
        if server.ends_with(']') {
            return Ok(format!("{server}:53"));
        }
    }

    let colons = server.matches(':').count();
    if colons == 1 && !server.contains(']') {
        return Ok(server.to_owned());
    }
    if colons >= 2 && !server.contains(']') {
        return Ok(format!("[{server}]:53"));
    }
    if !server.contains(':') {
        return Ok(format!("{server}:53"));
    }

    Ok(server.to_owned())
}

/// Split `host:port`, supporting bracketed IPv6 addresses.
pub fn split_host_port(address: &str) -> Result<(String, u16), DnsError> {
    let (host, port) = if let Some(rest) = address.strip_prefix('[') {
        let Some((host, tail)) = rest.split_once(']') else {
            return Err(DnsError::InvalidServer(address.to_owned()));
        };
        let Some(port) = tail.strip_prefix(':') else {
            return Err(DnsError::InvalidServer(address.to_owned()));
        };
        (host, port)
    } else {
        match address.rsplit_once(':') {
            Some((host, port)) => (host, port),
            None => return Err(DnsError::InvalidServer(address.to_owned())),
        }
    };

    if host.contains(':') && !address.starts_with('[') {
        return Err(DnsError::InvalidServer(address.to_owned()));
    }

    let port = port
        .parse::<u16>()
        .map_err(|_| DnsError::InvalidServer(address.to_owned()))?;

    Ok((host.to_owned(), port))
}

/// Sort addresses so that the preferred IP version comes first.
///
/// The sort is stable, so the original answer order is preserved within each
/// address family. `PreferIp::Auto` uses the local interface detection below.
pub fn sort_ips_by_preference(ips: &mut [IpAddr], prefer: PreferIp) {
    let effective = match prefer {
        PreferIp::Auto => {
            if local_has_ipv4() {
                PreferIp::V4
            } else {
                PreferIp::V6
            }
        }
        other => other,
    };

    ips.sort_by_key(|ip| match (effective, ip) {
        (PreferIp::V4, IpAddr::V4(_)) => 0,
        (PreferIp::V4, IpAddr::V6(_)) => 1,
        (PreferIp::V6, IpAddr::V4(_)) => 1,
        (PreferIp::V6, IpAddr::V6(_)) => 0,
        (PreferIp::Auto, _) => 0,
    });
}

/// Whether a non-loopback IPv4 interface exists.
///
/// The result is cached for the lifetime of the process, mirroring the Go
/// `preferV4Once` guard.
pub fn local_has_ipv4() -> bool {
    static HAS_IPV4: OnceLock<bool> = OnceLock::new();

    *HAS_IPV4.get_or_init(|| {
        if_addrs::get_if_addrs()
            .map(|interfaces| {
                interfaces.iter().any(|interface| {
                    let ip = interface.ip();
                    ip.is_ipv4() && !ip.is_loopback()
                })
            })
            .unwrap_or(false)
    })
}

/// Parsed response header and answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedResponse {
    pub id: u16,
    pub rcode: u8,
    pub truncated: bool,
    pub authoritative: bool,
    pub recursion_available: bool,
    pub answers: Vec<DnsRecord>,
}

/// A resource record returned by the DNS server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: u16,
    pub ttl: u32,
    /// `Some` for A/AAAA records, `None` for other record types.
    pub ip: Option<IpAddr>,
}

/// Parse a DNS response packet.
///
/// This is public so the packet parsing (compression pointers, A/AAAA,
/// NXDOMAIN, truncation) can be tested without a live server.
pub fn parse_response(packet: &[u8], expected_id: u16) -> Result<ParsedResponse, DnsError> {
    if packet.len() < 12 {
        return Err(DnsError::BadResponse(
            "packet shorter than header".to_owned(),
        ));
    }

    let id = u16::from_be_bytes([packet[0], packet[1]]);
    if id != expected_id {
        return Err(DnsError::BadResponse(format!(
            "response id {id} does not match query id {expected_id}"
        )));
    }

    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    let is_response = flags & 0x8000 != 0;
    if !is_response {
        return Err(DnsError::BadResponse("packet is not a response".to_owned()));
    }

    let opcode = (flags >> 11) & 0x0F;
    if opcode != 0 {
        return Err(DnsError::BadResponse(format!("unexpected opcode {opcode}")));
    }

    let truncated = flags & 0x0200 != 0;
    let authoritative = flags & 0x0400 != 0;
    let recursion_available = flags & 0x0080 != 0;
    let rcode = (flags & 0x000F) as u8;

    let question_count = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let answer_count = u16::from_be_bytes([packet[6], packet[7]]) as usize;

    let mut position = 12;

    for _ in 0..question_count {
        read_name(packet, &mut position)?;
        position = position
            .checked_add(4)
            .filter(|position| *position <= packet.len())
            .ok_or_else(|| DnsError::BadResponse("truncated question section".to_owned()))?;
    }

    let mut answers = Vec::with_capacity(answer_count.min(64));

    for _ in 0..answer_count {
        let name = read_name(packet, &mut position)?;

        if position + 10 > packet.len() {
            return Err(DnsError::BadResponse(
                "truncated resource record".to_owned(),
            ));
        }

        let record_type = u16::from_be_bytes([packet[position], packet[position + 1]]);
        let ttl = u32::from_be_bytes([
            packet[position + 4],
            packet[position + 5],
            packet[position + 6],
            packet[position + 7],
        ]);
        let data_len = u16::from_be_bytes([packet[position + 8], packet[position + 9]]) as usize;
        position += 10;

        if position + data_len > packet.len() {
            return Err(DnsError::BadResponse(
                "truncated resource record data".to_owned(),
            ));
        }

        let data = &packet[position..position + data_len];
        let ip = match (record_type, data_len) {
            (1, 4) => Some(IpAddr::V4(Ipv4Addr::new(
                data[0], data[1], data[2], data[3],
            ))),
            (28, 16) => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(data);
                Some(IpAddr::V6(Ipv6Addr::from(octets)))
            }
            _ => None,
        };

        answers.push(DnsRecord {
            name,
            record_type,
            ttl,
            ip,
        });

        position += data_len;
    }

    Ok(ParsedResponse {
        id,
        rcode,
        truncated,
        authoritative,
        recursion_available,
        answers,
    })
}

/// Build a standard recursive DNS query packet.
pub fn build_query(id: u16, name: &str, record_type: RecordType) -> Result<Vec<u8>, DnsError> {
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    packet.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    packet.extend_from_slice(&encode_name(name)?);
    packet.extend_from_slice(&record_type.as_u16().to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes()); // IN
    Ok(packet)
}

fn encode_name(name: &str) -> Result<Vec<u8>, DnsError> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() {
        return Err(DnsError::InvalidName(name.to_owned()));
    }

    let mut encoded = Vec::with_capacity(name.len() + 2);

    for label in name.split('.') {
        if label.is_empty() {
            return Err(DnsError::InvalidName(name.to_owned()));
        }
        if label.len() > 63 {
            return Err(DnsError::InvalidName(name.to_owned()));
        }
        encoded.push(label.len() as u8);
        encoded.extend_from_slice(label.as_bytes());
    }

    encoded.push(0);

    if encoded.len() > 255 {
        return Err(DnsError::InvalidName(name.to_owned()));
    }

    Ok(encoded)
}

fn read_name(packet: &[u8], position: &mut usize) -> Result<String, DnsError> {
    let mut labels: Vec<String> = Vec::new();
    let mut cursor = *position;
    let mut jumped = false;
    let mut hops = 0;

    loop {
        let Some(&length) = packet.get(cursor) else {
            return Err(DnsError::BadResponse("name exceeds packet".to_owned()));
        };

        if length & 0xC0 == 0xC0 {
            let Some(&low) = packet.get(cursor + 1) else {
                return Err(DnsError::BadResponse(
                    "truncated compression pointer".to_owned(),
                ));
            };
            let pointer = (((length & 0x3F) as usize) << 8) | low as usize;

            if !jumped {
                *position = cursor + 2;
                jumped = true;
            }

            hops += 1;
            if hops > 32 {
                return Err(DnsError::BadResponse("compression pointer loop".to_owned()));
            }
            cursor = pointer;
            continue;
        }

        if length & 0xC0 != 0 {
            return Err(DnsError::BadResponse("unsupported label type".to_owned()));
        }

        if length == 0 {
            if !jumped {
                *position = cursor + 1;
            }
            break;
        }

        let start = cursor + 1;
        let end = start + length as usize;
        let Some(label) = packet.get(start..end) else {
            return Err(DnsError::BadResponse("truncated label".to_owned()));
        };
        let label = std::str::from_utf8(label)
            .map_err(|_| DnsError::BadResponse("non-UTF-8 label".to_owned()))?;
        labels.push(label.to_owned());
        cursor = end;
    }

    Ok(labels.join("."))
}

fn query_id(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[0], packet[1]])
}

fn check_rcode(rcode: u8) -> Result<(), DnsError> {
    match rcode {
        0 => Ok(()),
        2 => Err(DnsError::ServFail),
        3 => Err(DnsError::NxDomain),
        5 => Err(DnsError::Refused),
        other => Err(DnsError::ResponseCode(other)),
    }
}

fn records_to_ips(records: Vec<DnsRecord>) -> Vec<IpAddr> {
    records.into_iter().filter_map(|record| record.ip).collect()
}

fn should_fallback(error: &DnsError) -> bool {
    !matches!(
        error,
        DnsError::NxDomain
            | DnsError::InvalidName(_)
            | DnsError::InvalidServer(_)
            | DnsError::InvalidPreference(_)
    )
}

fn prefer_error(current: Option<DnsError>, candidate: DnsError) -> DnsError {
    match current {
        // NXDOMAIN is the most specific answer: keep it over a transport
        // failure from the other address family.
        Some(DnsError::NxDomain) => DnsError::NxDomain,
        Some(error) => error,
        None => candidate,
    }
}

fn dedupe(ips: &mut Vec<IpAddr>) {
    let mut seen = Vec::with_capacity(ips.len());
    ips.retain(|ip| {
        if seen.contains(ip) {
            false
        } else {
            seen.push(*ip);
            true
        }
    });
}

async fn resolve_server_address(server: &str, timeout: Duration) -> Result<SocketAddr, DnsError> {
    let (host, port) = split_host_port(server)?;

    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }

    let lookup = tokio::net::lookup_host((host.as_str(), port));
    match tokio::time::timeout(timeout, lookup).await {
        Ok(Ok(mut addresses)) => addresses
            .next()
            .ok_or_else(|| DnsError::InvalidServer(server.to_owned())),
        Ok(Err(err)) => Err(DnsError::InvalidServer(format!("{server}: {err}"))),
        Err(_) => Err(DnsError::Timeout),
    }
}

fn to_ascii_name(name: &str) -> Result<String, DnsError> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() {
        return Err(DnsError::InvalidName(name.to_owned()));
    }
    idna::domain_to_ascii(name).map_err(|_| DnsError::InvalidName(name.to_owned()))
}

async fn query_udp(address: SocketAddr, packet: &[u8]) -> Result<Vec<u8>, DnsError> {
    let bind = if address.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(address).await?;
    socket.send(packet).await?;

    let mut buffer = vec![0u8; 4096];
    let length = socket.recv(&mut buffer).await?;
    buffer.truncate(length);
    Ok(buffer)
}

async fn query_tcp(address: SocketAddr, packet: &[u8]) -> Result<Vec<u8>, DnsError> {
    let mut stream = TcpStream::connect(address).await?;

    if packet.len() > u16::MAX as usize {
        return Err(DnsError::BadResponse("query too large for TCP".to_owned()));
    }

    stream
        .write_all(&(packet.len() as u16).to_be_bytes())
        .await?;
    stream.write_all(packet).await?;
    stream.flush().await?;

    let length = stream.read_u16().await? as usize;
    let mut response = vec![0u8; length];
    stream.read_exact(&mut response).await?;
    Ok(response)
}

fn next_id() -> u16 {
    static NEXT_ID: AtomicU16 = AtomicU16::new(0x9E37);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// DNS resolution errors, classified like the Go resolver's failure modes.
#[derive(Debug, Error)]
pub enum DnsError {
    #[error("invalid DNS server {0:?}")]
    InvalidServer(String),

    #[error("invalid DNS name {0:?}")]
    InvalidName(String),

    #[error("invalid prefer_ip_version value {0:?}: expected 4 or 6")]
    InvalidPreference(String),

    #[error("DNS query timed out")]
    Timeout,

    #[error("DNS I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("DNS name does not exist (NXDOMAIN)")]
    NxDomain,

    #[error("DNS server failure (SERVFAIL)")]
    ServFail,

    #[error("DNS query refused (REFUSED)")]
    Refused,

    #[error("DNS response was truncated")]
    Truncated,

    #[error("unexpected DNS response code {0}")]
    ResponseCode(u8),

    #[error("malformed DNS response: {0}")]
    BadResponse(String),

    #[error("no addresses found for {0}")]
    NoRecords(String),

    #[error("system resolver failed for {0}: {1}")]
    SystemLookup(String, String),

    #[error("all DNS servers failed: {0}")]
    AllServersFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer_packet(id: u16, flags: u16, record_type: u16, data: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(&id.to_be_bytes());
        packet.extend_from_slice(&flags.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        packet.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());

        // Question: example.com IN A/AAAA
        packet.extend_from_slice(&[7, b'e', b'x', b'a', b'm', b'p', b'l', b'e']);
        packet.extend_from_slice(&[3, b'c', b'o', b'm', 0]);
        packet.extend_from_slice(&record_type.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());

        // Answer name is a compression pointer to offset 12.
        packet.extend_from_slice(&[0xC0, 0x0C]);
        packet.extend_from_slice(&record_type.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&60u32.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(data);
        packet
    }

    #[test]
    fn normalize_dns_server_matches_go_rules() {
        let cases = [
            ("8.8.8.8", "8.8.8.8:53"),
            (" 8.8.8.8:5353 ", "8.8.8.8:5353"),
            ("dns.example.com", "dns.example.com:53"),
            ("dns.example.com:5353", "dns.example.com:5353"),
            ("2606:4700:4700::1111", "[2606:4700:4700::1111]:53"),
            ("[2606:4700:4700::1111]:5353", "[2606:4700:4700::1111]:5353"),
            ("[2606:4700:4700::1111]", "[2606:4700:4700::1111]:53"),
        ];

        for (input, expected) in cases {
            assert_eq!(normalize_dns_server(input).unwrap(), expected, "{input}");
        }

        assert!(matches!(
            normalize_dns_server("   "),
            Err(DnsError::InvalidServer(_))
        ));
    }

    #[test]
    fn split_host_port_handles_ipv4_ipv6_and_hostnames() {
        assert_eq!(
            split_host_port("1.1.1.1:53").unwrap(),
            ("1.1.1.1".to_owned(), 53)
        );
        assert_eq!(
            split_host_port("[::1]:5353").unwrap(),
            ("::1".to_owned(), 5353)
        );
        assert_eq!(
            split_host_port("dns.example.com:53").unwrap(),
            ("dns.example.com".to_owned(), 53)
        );
        assert!(split_host_port("::1:53").is_err());
        assert!(split_host_port("1.1.1.1").is_err());
    }

    #[test]
    fn parses_a_record_with_compression_pointer() {
        let packet = answer_packet(7, 0x8180, 1, &[93, 184, 216, 34]);
        let parsed = parse_response(&packet, 7).unwrap();

        assert_eq!(parsed.rcode, 0);
        assert!(!parsed.truncated);
        assert!(parsed.recursion_available);
        assert_eq!(parsed.answers.len(), 1);
        assert_eq!(parsed.answers[0].name, "example.com");
        assert_eq!(parsed.answers[0].ttl, 60);
        assert_eq!(
            parsed.answers[0].ip,
            Some(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)))
        );
    }

    #[test]
    fn parses_aaaa_record() {
        let mut data = [0u8; 16];
        data[0] = 0x20;
        data[1] = 0x01;
        data[2] = 0x0d;
        data[3] = 0xb8;
        data[15] = 1;

        let packet = answer_packet(8, 0x8180, 28, &data);
        let parsed = parse_response(&packet, 8).unwrap();

        assert_eq!(parsed.answers.len(), 1);
        assert_eq!(
            parsed.answers[0].ip,
            Some(IpAddr::V6("2001:db8::1".parse().unwrap()))
        );
    }

    #[test]
    fn parses_nxdomain_and_truncation_flags() {
        let nxdomain = answer_packet(9, 0x8183, 1, &[]);
        let parsed = parse_response(&nxdomain, 9).unwrap();
        assert_eq!(parsed.rcode, 3);
        assert!(matches!(check_rcode(parsed.rcode), Err(DnsError::NxDomain)));

        let truncated = answer_packet(10, 0x8380, 1, &[]);
        let parsed = parse_response(&truncated, 10).unwrap();
        assert!(parsed.truncated);
        assert_eq!(parsed.rcode, 0);
    }

    #[test]
    fn rejects_bad_id_and_truncated_packets() {
        let packet = answer_packet(11, 0x8180, 1, &[1, 2, 3, 4]);

        assert!(matches!(
            parse_response(&packet, 12),
            Err(DnsError::BadResponse(_))
        ));
        assert!(matches!(
            parse_response(&packet[..10], 11),
            Err(DnsError::BadResponse(_))
        ));
    }

    #[test]
    fn builds_queries_with_expected_wire_format() {
        let packet = build_query(0x1234, "example.com", RecordType::A).unwrap();
        assert_eq!(&packet[..2], &[0x12, 0x34]);
        assert_eq!(&packet[2..4], &[0x01, 0x00]);
        assert_eq!(&packet[4..6], &[0x00, 0x01]);
        assert_eq!(&packet[12..], b"\x07example\x03com\x00\x00\x01\x00\x01");

        assert!(matches!(
            build_query(1, "a..b", RecordType::A),
            Err(DnsError::InvalidName(_))
        ));
    }

    #[test]
    fn sorts_addresses_by_preferred_family_stably() {
        let v4a: IpAddr = "1.1.1.1".parse().unwrap();
        let v4b: IpAddr = "2.2.2.2".parse().unwrap();
        let v6a: IpAddr = "2606:4700::1111".parse().unwrap();
        let v6b: IpAddr = "2606:4700::1001".parse().unwrap();

        let mut ips = vec![v6a, v4a, v6b, v4b];
        sort_ips_by_preference(&mut ips, PreferIp::V4);
        assert_eq!(ips, vec![v4a, v4b, v6a, v6b]);

        let mut ips = vec![v6a, v4a, v6b, v4b];
        sort_ips_by_preference(&mut ips, PreferIp::V6);
        assert_eq!(ips, vec![v6a, v6b, v4a, v4b]);
    }

    #[test]
    fn parses_prefer_ip_values() {
        assert_eq!(PreferIp::parse("").unwrap(), PreferIp::Auto);
        assert_eq!(PreferIp::parse("4").unwrap(), PreferIp::V4);
        assert_eq!(PreferIp::parse("6").unwrap(), PreferIp::V6);
        assert!(matches!(
            PreferIp::parse("5"),
            Err(DnsError::InvalidPreference(_))
        ));
    }

    #[test]
    fn default_config_matches_go_server_list() {
        let config = ResolverConfig::default();
        assert_eq!(config.fallback_servers.len(), BUILTIN_DNS_SERVERS.len());
        assert_eq!(config.fallback_servers[0], "[2606:4700:4700::1111]:53");
        assert!(config.system_fallback);
    }
}
