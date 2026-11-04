//! DNS and HTTP support for the Komari agent.
//!
//! The crate contains two independent pieces:
//!
//! * [`dns`] — a small DNS client with custom server, fallback list, system
//!   resolver fallback and IPv4/IPv6 preference ordering.
//! * [`http_client`] — an injectable HTTP transport (`HttpClient`) and the
//!   default `reqwest` implementation (`ReqwestHttpClient`) that resolves
//!   through [`dns::DnsResolver`].
//!
//! Both modules are intentionally free of WebSocket and metric collection
//! concerns; those live in their own crates.

pub mod dns;
pub mod http_client;
pub mod idna;

pub use dns::{
    BUILTIN_DNS_SERVERS, DnsError, DnsHeader, DnsRecord, DnsResolver, ExpectedQuestion,
    ParsedResponse, PreferIp, RecordType, ResolverConfig, build_query, local_has_ipv4,
    normalize_dns_server, parse_header, parse_response, sort_ips_by_preference, split_host_port,
};
pub use http_client::{
    DiscoveryError, DnsReqwestResolver, HttpClient, HttpRequest, HttpResponse, ReqwestHttpClient,
    build_reqwest_client,
};
pub use idna::{IdnaError, convert_host_to_ascii, convert_url_to_ascii};
