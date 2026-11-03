//! Discovery support for the Komari agent.
//!
//! The crate contains two independent pieces:
//!
//! * [`dns`] — a small DNS client with custom server, fallback list, system
//!   resolver fallback and IPv4/IPv6 preference ordering.
//! * [`autodiscovery`] — registration with the panel via an auto discovery key
//!   and persistence of the returned credentials.
//!
//! Both modules are intentionally free of WebSocket and metric collection
//! concerns; those live in their own crates.

pub mod dns;

pub use dns::{
    BUILTIN_DNS_SERVERS, DnsError, DnsRecord, DnsResolver, ParsedResponse, PreferIp, RecordType,
    ResolverConfig, local_has_ipv4, normalize_dns_server, parse_response, sort_ips_by_preference,
    split_host_port,
};
