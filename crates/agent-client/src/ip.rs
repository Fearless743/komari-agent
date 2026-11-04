//! IP address selection for the basic information payload.
//!
//! `agent-monitor` owns every metric in the periodic report and the basic-info
//! `disk_total`/`gpu_name` fields, but it does not collect IP addresses. This
//! module mirrors Go's `monitoring.GetIPAddress` priority:
//!
//! 1. addresses from the configured NICs (when `get_ip_addr_from_nic` is set);
//! 2. `custom_ipv4`/`custom_ipv6` per address family;
//! 3. the external lookup services, only for families that are still empty.
//!
//! On Unix interfaces are enumerated with `getifaddrs` and `IFF_UP`, matching
//! Go's `net.FlagUp`; `if-addrs::is_oper_up` is only used on non-Unix platforms
//! where it is an approximation (D-011).
//!
//! External lookups use a blocking `reqwest` client because the provider runs
//! on `spawn_blocking`. The client resolves through [`DnsResolver`], pins the
//! socket family to IPv4 or IPv6 (Go dials `tcp4`/`tcp6`), honours the usual
//! proxy environment variables and never logs tokens or response bodies.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use agent_discovery::DnsResolver;
use agent_monitor::NicFilter;
use regex::Regex;
use reqwest::blocking::ClientBuilder;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{NoProxy, Proxy};
use tracing::{debug, warn};

/// Go's user agent for the external IP services.
pub const EXTERNAL_IP_USER_AGENT: &str = "curl/8.0.1";

/// Go's per-request timeout for the external IP services.
pub const EXTERNAL_IP_TIMEOUT: Duration = Duration::from_secs(15);

/// IPv4 endpoints in Go's order (`monitoring.GetIPv4Address`).
pub const GO_IPV4_ENDPOINTS: [&str; 7] = [
    "https://www.visa.cn/cdn-cgi/trace",
    "https://www.qualcomm.cn/cdn-cgi/trace",
    "https://www.toutiao.com/stream/widget/local_weather/data/",
    "https://edge-ip.html.zone/geo",
    "https://vercel-ip.html.zone/geo",
    "http://ipv4.ip.sb",
    "https://api.ipify.org?format=json",
];

/// IPv6 endpoints in Go's order (`monitoring.GetIPv6Address`).
pub const GO_IPV6_ENDPOINTS: [&str; 4] = [
    "https://v6.ip.zxinc.org/info.php?type=json",
    "https://api6.ipify.org?format=json",
    "https://ipv6.icanhazip.com",
    "http://api-ipv6.ip.sb/geoip",
];

static IPV4_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}").expect("valid IPv4 regex"));

// Kept byte-for-byte equivalent to Go's IPv6 expression so both agents accept
// the same bodies (including compressed forms).
static IPV6_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(([0-9A-Fa-f]{1,4}:){7})([0-9A-Fa-f]{1,4})|(([0-9A-Fa-f]{1,4}:){1,6}:)(([0-9A-Fa-f]{1,4}:){0,4})([0-9A-Fa-f]{0,4})",
    )
    .expect("valid IPv6 regex")
});

/// Address family used by an external lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpFamily {
    V4,
    V6,
}

impl IpFamily {
    fn matches(self, ip: IpAddr) -> bool {
        matches!(
            (self, ip),
            (IpFamily::V4, IpAddr::V4(_)) | (IpFamily::V6, IpAddr::V6(_))
        )
    }

    fn label(self) -> &'static str {
        match self {
            IpFamily::V4 => "IPv4",
            IpFamily::V6 => "IPv6",
        }
    }
}

/// Options for IP address selection.
#[derive(Clone, Debug, Default)]
pub struct IpOptions {
    pub include_nics: String,
    pub exclude_nics: String,
    pub custom_ipv4: String,
    pub custom_ipv6: String,
    pub get_ip_addr_from_nic: bool,
    /// External lookup endpoints. Defaults to Go's lists; tests replace them
    /// with local mock servers.
    pub external: ExternalIpConfig,
}

/// Endpoint lists, timeout and user agent for the external IP services.
#[derive(Clone, Debug)]
pub struct ExternalIpConfig {
    pub ipv4_urls: Vec<String>,
    pub ipv6_urls: Vec<String>,
    pub timeout: Duration,
    pub user_agent: String,
}

impl Default for ExternalIpConfig {
    fn default() -> Self {
        Self {
            ipv4_urls: GO_IPV4_ENDPOINTS
                .iter()
                .map(|url| (*url).to_owned())
                .collect(),
            ipv6_urls: GO_IPV6_ENDPOINTS
                .iter()
                .map(|url| (*url).to_owned())
                .collect(),
            timeout: EXTERNAL_IP_TIMEOUT,
            user_agent: EXTERNAL_IP_USER_AGENT.to_owned(),
        }
    }
}

/// Proxy environment values captured once when the lookup is created.
///
/// `Debug` redacts the URLs because they may contain credentials.
#[derive(Clone, Default)]
pub struct ProxySettings {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
}

impl fmt::Debug for ProxySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxySettings")
            .field("http", &self.http.as_ref().map(|_| "<set>"))
            .field("https", &self.https.as_ref().map(|_| "<set>"))
            .field("no_proxy", &self.no_proxy.as_deref())
            .finish()
    }
}

impl ProxySettings {
    /// Read the conventional proxy variables, matching Go's
    /// `http.ProxyFromEnvironment` (plus `ALL_PROXY` as a fallback).
    pub fn from_env() -> Self {
        Self::from_env_lookup(&|key| std::env::var(key).ok())
    }

    /// Testable variant of [`ProxySettings::from_env`].
    pub fn from_env_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let all = env_value(lookup, &["ALL_PROXY", "all_proxy"]);
        Self {
            http: env_value(lookup, &["HTTP_PROXY", "http_proxy"]).or_else(|| all.clone()),
            https: env_value(lookup, &["HTTPS_PROXY", "https_proxy"]).or(all),
            no_proxy: env_value(lookup, &["NO_PROXY", "no_proxy"]),
        }
    }
}

fn env_value(lookup: &dyn Fn(&str) -> Option<String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| lookup(key).filter(|value| !value.trim().is_empty()))
}

/// External IPv4/IPv6 lookup with a custom resolver and proxy settings.
pub struct ExternalIpLookup {
    config: ExternalIpConfig,
    resolver: DnsResolver,
    proxy: ProxySettings,
}

impl ExternalIpLookup {
    /// Build a lookup using the process proxy environment.
    pub fn new(config: ExternalIpConfig, resolver: DnsResolver) -> Self {
        Self {
            config,
            resolver,
            proxy: ProxySettings::from_env(),
        }
    }

    /// Override the proxy settings (used by tests and future configuration).
    pub fn with_proxy(mut self, proxy: ProxySettings) -> Self {
        self.proxy = proxy;
        self
    }

    /// Query the endpoints for one family, returning the first match.
    ///
    /// Transport errors, timeouts and bodies without a matching address move
    /// on to the next endpoint. Like Go, a non-2xx response body is still
    /// scanned; the response body is never logged.
    pub fn lookup(&self, family: IpFamily) -> Option<String> {
        let urls = match family {
            IpFamily::V4 => &self.config.ipv4_urls,
            IpFamily::V6 => &self.config.ipv6_urls,
        };
        if urls.is_empty() {
            return None;
        }

        let client = match self.build_client(family) {
            Ok(client) => client,
            Err(err) => {
                warn!(family = family.label(), %err, "failed to build external IP client");
                return None;
            }
        };

        for url in urls {
            debug!(family = family.label(), endpoint = %url, "querying external IP endpoint");
            let body = match client.get(url).send() {
                Ok(response) => match response.text() {
                    Ok(body) => body,
                    Err(err) => {
                        debug!(family = family.label(), endpoint = %url, %err, "external IP body read failed");
                        continue;
                    }
                },
                Err(err) => {
                    debug!(family = family.label(), endpoint = %url, %err, "external IP request failed");
                    continue;
                }
            };

            if let Some(ip) = extract_ip(family, &body) {
                debug!(family = family.label(), %ip, "external IP lookup succeeded");
                return Some(ip);
            }
        }

        None
    }

    fn build_client(&self, family: IpFamily) -> Result<reqwest::blocking::Client, reqwest::Error> {
        let timeout = if self.config.timeout.is_zero() {
            EXTERNAL_IP_TIMEOUT
        } else {
            self.config.timeout
        };
        let local_address = match family {
            IpFamily::V4 => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpFamily::V6 => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };

        let builder = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .user_agent(self.config.user_agent.clone())
            .dns_resolver(Arc::new(FamilyResolver::new(self.resolver.clone(), family)))
            // Go dials `tcp4`/`tcp6`. Filtering DNS answers covers hostnames;
            // the local address also rejects the other family for literals.
            .local_address(local_address);

        apply_proxy_settings(builder, &self.proxy).build()
    }
}

/// Resolve IPv4/IPv6 addresses using Go's priority rules.
pub fn addresses(options: &IpOptions) -> (String, String) {
    let lookup = ExternalIpLookup::new(options.external.clone(), DnsResolver::system_only());
    addresses_with(options, &lookup)
}

/// Resolve addresses with an explicit external lookup (custom DNS/proxy).
pub fn addresses_with(options: &IpOptions, lookup: &ExternalIpLookup) -> (String, String) {
    let (nic_ipv4, nic_ipv6) = if options.get_ip_addr_from_nic {
        nic_addresses(options)
    } else {
        (String::new(), String::new())
    };

    select_addresses(
        &options.custom_ipv4,
        &options.custom_ipv6,
        nic_ipv4,
        nic_ipv6,
        |family| lookup.lookup(family),
    )
}

/// Go's priority: NIC addresses (when enabled and found) win over everything;
/// otherwise each family falls back from custom to the external service.
fn select_addresses<F>(
    custom_ipv4: &str,
    custom_ipv6: &str,
    nic_ipv4: String,
    nic_ipv6: String,
    mut external: F,
) -> (String, String)
where
    F: FnMut(IpFamily) -> Option<String>,
{
    if !nic_ipv4.is_empty() || !nic_ipv6.is_empty() {
        return (nic_ipv4, nic_ipv6);
    }

    let ipv4 = if custom_ipv4.is_empty() {
        external(IpFamily::V4).unwrap_or_default()
    } else {
        custom_ipv4.to_owned()
    };
    let ipv6 = if custom_ipv6.is_empty() {
        external(IpFamily::V6).unwrap_or_default()
    } else {
        custom_ipv6.to_owned()
    };

    (ipv4, ipv6)
}

fn extract_ip(family: IpFamily, body: &str) -> Option<String> {
    let regex = match family {
        IpFamily::V4 => &IPV4_RE,
        IpFamily::V6 => &IPV6_RE,
    };
    regex.find(body).map(|matched| matched.as_str().to_owned())
}

/// reqwest resolver that keeps only the requested address family.
struct FamilyResolver {
    resolver: DnsResolver,
    family: IpFamily,
}

impl FamilyResolver {
    fn new(resolver: DnsResolver, family: IpFamily) -> Self {
        Self { resolver, family }
    }
}

impl Resolve for FamilyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.resolver.clone();
        let family = self.family;
        let host = name.as_str().to_owned();

        Box::pin(async move {
            let addresses = resolver
                .resolve_host_unsorted(&host)
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?;

            let addresses = filter_family(addresses, family);
            if addresses.is_empty() {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no {} records for {host}", family.label()),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            let addresses: Vec<SocketAddr> = addresses
                .into_iter()
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

fn filter_family(addresses: Vec<IpAddr>, family: IpFamily) -> Vec<IpAddr> {
    addresses
        .into_iter()
        .filter(|ip| family.matches(*ip))
        .collect()
}

fn apply_proxy_settings(mut builder: ClientBuilder, settings: &ProxySettings) -> ClientBuilder {
    if let Some(value) = settings.http.as_deref() {
        match Proxy::http(normalize_proxy_url(value)) {
            Ok(proxy) => builder = builder.proxy(proxy_with_no_proxy(proxy, settings)),
            // The parse error can contain the proxy URL (and its credentials),
            // so only record that the value was ignored.
            Err(_) => debug!("ignoring invalid HTTP_PROXY"),
        }
    }
    if let Some(value) = settings.https.as_deref() {
        match Proxy::https(normalize_proxy_url(value)) {
            Ok(proxy) => builder = builder.proxy(proxy_with_no_proxy(proxy, settings)),
            Err(_) => debug!("ignoring invalid HTTPS_PROXY"),
        }
    }
    builder
}

fn proxy_with_no_proxy(proxy: Proxy, settings: &ProxySettings) -> Proxy {
    match settings
        .no_proxy
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .and_then(NoProxy::from_string)
    {
        Some(no_proxy) => proxy.no_proxy(Some(no_proxy)),
        None => proxy,
    }
}

/// Go accepts `host:port` proxies; reqwest needs a scheme.
fn normalize_proxy_url(value: &str) -> String {
    let value = value.trim();
    if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    }
}

#[cfg(unix)]
fn nic_addresses(options: &IpOptions) -> (String, String) {
    use std::ffi::CStr;

    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs either fails without touching `head` or returns a
    // linked list released exactly once with freeifaddrs.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return (String::new(), String::new());
    }

    let filter = NicFilter::new(&options.include_nics, &options.exclude_nics);
    let mut ipv4 = String::new();
    let mut ipv6 = String::new();
    let mut current = head;

    while !current.is_null() {
        // SAFETY: `current` is a valid node from the getifaddrs list.
        let entry = unsafe { &*current };

        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_name is a NUL-terminated string owned by the ifaddrs
            // entry for the duration of the list.
            let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy();
            // SAFETY: ifa_addr points to a sockaddr whose family decides which
            // concrete layout is valid.
            let family = unsafe { (*entry.ifa_addr).sa_family as i32 };

            if filter.should_include(&name)
                && interface_flags_allow_ip(entry.ifa_flags as libc::c_uint)
            {
                if family == libc::AF_INET {
                    // SAFETY: AF_INET guarantees sockaddr_in layout.
                    let sockaddr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                    let address = Ipv4Addr::from(u32::from_be(sockaddr.sin_addr.s_addr));
                    if ipv4.is_empty() && !address.is_loopback() {
                        ipv4 = address.to_string();
                    }
                } else if family == libc::AF_INET6 {
                    // SAFETY: AF_INET6 guarantees sockaddr_in6 layout.
                    let sockaddr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                    let address = Ipv6Addr::from(sockaddr.sin6_addr.s6_addr);
                    if ipv6.is_empty() && !address.is_loopback() && !address.is_unicast_link_local()
                    {
                        ipv6 = address.to_string();
                    }
                }
            }
        }

        current = entry.ifa_next;
    }

    // SAFETY: head was returned by getifaddrs and is released once.
    unsafe { libc::freeifaddrs(head) };
    (ipv4, ipv6)
}

#[cfg(not(unix))]
fn nic_addresses(options: &IpOptions) -> (String, String) {
    let filter = NicFilter::new(&options.include_nics, &options.exclude_nics);
    let mut ipv4 = String::new();
    let mut ipv6 = String::new();

    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if !filter.should_include(&interface.name) || !interface.is_oper_up() {
                continue;
            }
            match interface.ip() {
                std::net::IpAddr::V4(address) => {
                    if ipv4.is_empty() && !address.is_loopback() {
                        ipv4 = address.to_string();
                    }
                }
                std::net::IpAddr::V6(address) => {
                    if ipv6.is_empty() && !address.is_loopback() && !address.is_unicast_link_local()
                    {
                        ipv6 = address.to_string();
                    }
                }
            }
        }
    }

    (ipv4, ipv6)
}

/// Go uses `net.FlagUp` (IFF_UP), not IFF_RUNNING. An administratively up
/// interface without carrier is still eligible.
#[cfg(unix)]
fn interface_flags_allow_ip(flags: libc::c_uint) -> bool {
    let up = libc::IFF_UP as libc::c_uint;
    let loopback = libc::IFF_LOOPBACK as libc::c_uint;
    (flags & up) != 0 && (flags & loopback) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nic_filter_uses_a03_semantics() {
        let filter = NicFilter::new("", "");
        for name in ["lo", "veth123", "docker0", "br-abc", "cni0", "tap0"] {
            assert!(!filter.should_include(name), "{name}");
        }
        assert!(filter.should_include("eth0"));
        assert!(filter.should_include("enp1s0"));

        let filter = NicFilter::new("eth*", "");
        assert!(filter.should_include("eth0"));
        assert!(!filter.should_include("enp1s0"));

        let filter = NicFilter::new("", "enp*");
        assert!(!filter.should_include("enp1s0"));
        assert!(filter.should_include("eth0"));
    }

    #[test]
    fn whitespace_only_include_list_excludes_everything() {
        // A03/Go treat " " as a non-empty include set with no usable patterns.
        let filter = NicFilter::new(" ", "");
        assert!(!filter.should_include("eth0"));
        assert!(!filter.should_include("enp1s0"));
    }

    #[test]
    fn custom_addresses_are_used_when_nic_lookup_is_disabled() {
        let options = IpOptions {
            custom_ipv4: "203.0.113.7".to_owned(),
            custom_ipv6: "2001:db8::7".to_owned(),
            get_ip_addr_from_nic: false,
            ..IpOptions::default()
        };
        // Both custom values are set, so no external lookup is attempted.
        let lookup = ExternalIpLookup::new(
            ExternalIpConfig {
                ipv4_urls: Vec::new(),
                ipv6_urls: Vec::new(),
                ..ExternalIpConfig::default()
            },
            DnsResolver::system_only(),
        );
        assert_eq!(
            addresses_with(&options, &lookup),
            ("203.0.113.7".to_owned(), "2001:db8::7".to_owned())
        );
    }

    #[test]
    fn nic_addresses_win_over_custom_values_like_go() {
        assert_eq!(
            select_addresses(
                "custom4",
                "custom6",
                "nic4".to_owned(),
                String::new(),
                |_| { panic!("external lookup must not run when a NIC address exists") }
            ),
            ("nic4".to_owned(), String::new())
        );
        assert_eq!(
            select_addresses(
                "custom4",
                "custom6",
                String::new(),
                String::new(),
                |_| panic!("external lookup must not run when custom values exist"),
            ),
            ("custom4".to_owned(), "custom6".to_owned())
        );
    }

    #[test]
    fn external_lookup_runs_only_for_empty_families() {
        let mut requested = Vec::new();
        let (ipv4, ipv6) =
            select_addresses("custom4", "", String::new(), String::new(), |family| {
                requested.push(family);
                Some(match family {
                    IpFamily::V4 => "198.51.100.4".to_owned(),
                    IpFamily::V6 => "2001:db8::4".to_owned(),
                })
            });
        assert_eq!(ipv4, "custom4");
        assert_eq!(ipv6, "2001:db8::4");
        assert_eq!(requested, vec![IpFamily::V6]);
    }

    #[test]
    fn default_endpoints_match_go() {
        let config = ExternalIpConfig::default();
        assert_eq!(
            config.ipv4_urls,
            GO_IPV4_ENDPOINTS
                .iter()
                .map(|url| (*url).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            config.ipv6_urls,
            GO_IPV6_ENDPOINTS
                .iter()
                .map(|url| (*url).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(config.timeout, Duration::from_secs(15));
        assert_eq!(config.user_agent, "curl/8.0.1");
    }

    #[test]
    fn ipv4_regex_matches_go() {
        let body = "fl=abc\nh=www.visa.cn\nip=203.0.113.9\nts=1\n";
        assert_eq!(
            extract_ip(IpFamily::V4, body).as_deref(),
            Some("203.0.113.9")
        );
        assert_eq!(
            extract_ip(IpFamily::V4, r#"{"ip":"198.51.100.7"}"#).as_deref(),
            Some("198.51.100.7")
        );
        assert!(extract_ip(IpFamily::V4, "no address here").is_none());
    }

    #[test]
    fn ipv6_regex_matches_full_and_compressed_forms() {
        assert_eq!(
            extract_ip(IpFamily::V6, "2001:db8:0:0:0:0:0:1\n").as_deref(),
            Some("2001:db8:0:0:0:0:0:1")
        );
        assert_eq!(
            extract_ip(IpFamily::V6, r#"{"ip":"2001:db8::1"}"#).as_deref(),
            Some("2001:db8::1")
        );
        assert!(extract_ip(IpFamily::V6, "203.0.113.9").is_none());
    }

    #[test]
    fn family_filter_keeps_only_the_requested_version() {
        let addresses = vec![
            "203.0.113.9".parse().unwrap(),
            "2001:db8::1".parse().unwrap(),
            "198.51.100.7".parse().unwrap(),
        ];
        assert_eq!(
            filter_family(addresses.clone(), IpFamily::V4),
            vec![
                "203.0.113.9".parse::<IpAddr>().unwrap(),
                "198.51.100.7".parse::<IpAddr>().unwrap()
            ]
        );
        assert_eq!(
            filter_family(addresses, IpFamily::V6),
            vec!["2001:db8::1".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn proxy_environment_uses_http_https_and_all_proxy() {
        let settings = ProxySettings::from_env_lookup(&|key| match key {
            "HTTP_PROXY" => Some("http://proxy.example:8080".to_owned()),
            "HTTPS_PROXY" => Some("http://secure.example:8443".to_owned()),
            "NO_PROXY" => Some("localhost,127.0.0.1".to_owned()),
            _ => None,
        });
        assert_eq!(settings.http.as_deref(), Some("http://proxy.example:8080"));
        assert_eq!(
            settings.https.as_deref(),
            Some("http://secure.example:8443")
        );
        assert_eq!(settings.no_proxy.as_deref(), Some("localhost,127.0.0.1"));

        let settings = ProxySettings::from_env_lookup(&|key| match key {
            "ALL_PROXY" => Some("http://all.example:3128".to_owned()),
            _ => None,
        });
        assert_eq!(settings.http.as_deref(), Some("http://all.example:3128"));
        assert_eq!(settings.https.as_deref(), Some("http://all.example:3128"));
    }

    #[test]
    fn proxy_urls_without_a_scheme_are_accepted() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:1080"),
            "socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn proxy_debug_redacts_credentials() {
        let settings = ProxySettings {
            http: Some("http://user:secret@proxy.example:8080".to_owned()),
            https: None,
            no_proxy: None,
        };
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("secret"));
        assert!(rendered.contains("<set>"));
    }

    #[cfg(unix)]
    #[test]
    fn iff_up_without_running_still_counts() {
        let up = libc::IFF_UP as libc::c_uint;
        let running = libc::IFF_RUNNING as libc::c_uint;
        let loopback = libc::IFF_LOOPBACK as libc::c_uint;

        assert!(interface_flags_allow_ip(up));
        assert!(interface_flags_allow_ip(up | running));
        assert!(!interface_flags_allow_ip(running));
        assert!(!interface_flags_allow_ip(up | loopback));
    }
}
