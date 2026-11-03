//! Internationalized domain name helpers.
//!
//! Mirrors `komari-agent/utils/idna.go`. Endpoints containing non-ASCII
//! domains are converted to Punycode before a request is created. IP
//! addresses are never converted.

use std::net::IpAddr;

use thiserror::Error;
use url::Url;

/// Errors produced while converting an IDN.
#[derive(Debug, Error)]
pub enum IdnaError {
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("URL has no host: {0}")]
    MissingHost(String),

    #[error("invalid internationalized domain {0:?}")]
    InvalidDomain(String),
}

/// Convert a bare hostname (optionally with a port) to ASCII.
///
/// Bracketed IPv6 addresses, unbracketed IPv6 addresses and IPv4 addresses
/// are returned unchanged.
pub fn convert_host_to_ascii(host: &str) -> Result<String, IdnaError> {
    let (hostname, suffix) = split_host_suffix(host);

    if hostname.parse::<IpAddr>().is_ok() {
        return Ok(host.to_owned());
    }

    let ascii = idna::domain_to_ascii(hostname)
        .map_err(|_| IdnaError::InvalidDomain(hostname.to_owned()))?;

    Ok(format!("{ascii}{suffix}"))
}

/// Convert the host part of a URL to ASCII.
///
/// The rest of the URL (scheme, path, query) is preserved by the `url` crate.
pub fn convert_url_to_ascii(url_str: &str) -> Result<String, IdnaError> {
    let mut url = Url::parse(url_str)?;
    let host = url
        .host_str()
        .ok_or_else(|| IdnaError::MissingHost(url_str.to_owned()))?
        .to_owned();

    if host.parse::<IpAddr>().is_ok() {
        return Ok(url.to_string());
    }

    let ascii =
        idna::domain_to_ascii(&host).map_err(|_| IdnaError::InvalidDomain(host.to_owned()))?;
    url.set_host(Some(&ascii))
        .map_err(|_| IdnaError::InvalidDomain(host))?;

    Ok(url.to_string())
}

fn split_host_suffix(host: &str) -> (&str, &str) {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some((hostname, tail)) = rest.split_once(']') {
            return (hostname, tail);
        }
        return (host, "");
    }

    if host.matches(':').count() > 1 {
        // Unbracketed IPv6 address, no port.
        return (host, "");
    }

    if let Some((hostname, _port)) = host.split_once(':') {
        let offset = hostname.len();
        return (&host[..offset], &host[offset..]);
    }

    (host, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_idn_hosts_and_preserves_ports() {
        assert_eq!(
            convert_host_to_ascii("中文.com").unwrap(),
            "xn--fiq228c.com"
        );
        assert_eq!(
            convert_host_to_ascii("中文.com:8080").unwrap(),
            "xn--fiq228c.com:8080"
        );
        assert_eq!(convert_host_to_ascii("1.1.1.1:53").unwrap(), "1.1.1.1:53");
        assert_eq!(convert_host_to_ascii("[::1]:53").unwrap(), "[::1]:53");
        assert_eq!(
            convert_host_to_ascii("2606:4700::1111").unwrap(),
            "2606:4700::1111"
        );
    }

    #[test]
    fn converts_idn_urls() {
        let converted = convert_url_to_ascii("https://中文.com/api").unwrap();
        assert_eq!(converted, "https://xn--fiq228c.com/api");

        let ipv4 = convert_url_to_ascii("https://1.1.1.1:8443/api").unwrap();
        assert_eq!(ipv4, "https://1.1.1.1:8443/api");

        assert!(matches!(
            convert_url_to_ascii("not a url"),
            Err(IdnaError::InvalidUrl(_))
        ));
    }
}
