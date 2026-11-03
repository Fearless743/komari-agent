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

pub mod autodiscovery;
pub mod dns;
pub mod idna;

pub use autodiscovery::{
    AUTO_DISCOVERY_FILE, AutoDiscovery, AutoDiscoveryConfig, AutoDiscoveryOptions, DiscoveryError,
    DiscoveryResult, HttpClient, HttpRequest, HttpResponse, ReqwestHttpClient, default_config_path,
};
pub use dns::{
    BUILTIN_DNS_SERVERS, DnsError, DnsHeader, DnsRecord, DnsResolver, ExpectedQuestion,
    ParsedResponse, PreferIp, RecordType, ResolverConfig, build_query, local_has_ipv4,
    normalize_dns_server, parse_header, parse_response, sort_ips_by_preference, split_host_port,
};
pub use idna::{IdnaError, convert_host_to_ascii, convert_url_to_ascii};
