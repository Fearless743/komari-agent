//! Dashboard URL construction.
//!
//! Mirrors the Go helpers:
//!
//! * [`build_ws_endpoint`] — `{endpoint}/api/clients/report?token={token}` with
//!   `http`/`https` rewritten to `ws`/`wss`, one trailing slash removed and the
//!   host converted to Punycode;
//! * [`build_basic_info_endpoint`] —
//!   `{endpoint}/api/clients/uploadBasicInfo?token={token}`.
//!
//! The token is appended verbatim, exactly like Go's string concatenation.
//! Komari tokens are UUIDs, so percent-encoding would only make the URL differ
//! from what the Go agent sends.

use agent_discovery::convert_url_to_ascii;
use thiserror::Error;
use url::Url;

/// WebSocket report path.
pub const REPORT_PATH: &str = "/api/clients/report";

/// Basic information upload path.
pub const BASIC_INFO_PATH: &str = "/api/clients/uploadBasicInfo";

/// URL construction failures.
#[derive(Debug, Error)]
pub enum EndpointError {
    #[error("endpoint is empty")]
    Empty,

    #[error("invalid endpoint {url:?}: {source}")]
    Invalid {
        url: String,
        #[source]
        source: url::ParseError,
    },

    #[error("unsupported endpoint scheme {0:?}: expected http or https")]
    UnsupportedScheme(String),

    #[error("failed to convert endpoint IDN: {0}")]
    Idn(#[from] agent_discovery::IdnaError),
}

/// Build the WebSocket report endpoint.
pub fn build_ws_endpoint(endpoint: &str, token: &str) -> Result<String, EndpointError> {
    let (base, scheme) = normalize_endpoint(endpoint)?;
    let base = match scheme {
        "http" => format!("ws{}", &base["http".len()..]),
        _ => format!("wss{}", &base["https".len()..]),
    };
    let url = format!("{base}{REPORT_PATH}?token={token}");
    Ok(convert_url_to_ascii(&url)?)
}

/// Build the basic information upload endpoint.
pub fn build_basic_info_endpoint(endpoint: &str, token: &str) -> Result<String, EndpointError> {
    let (base, _) = normalize_endpoint(endpoint)?;
    let url = format!("{base}{BASIC_INFO_PATH}?token={token}");
    Ok(convert_url_to_ascii(&url)?)
}

/// Validate and normalize an `http`/`https` endpoint.
///
/// The returned string has no trailing slash and keeps any path prefix, so
/// `https://panel.example/komari/` becomes
/// `https://panel.example/komari/api/clients/...`.
fn normalize_endpoint(endpoint: &str) -> Result<(String, &'static str), EndpointError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(EndpointError::Empty);
    }

    let trimmed = endpoint.trim_end_matches('/');
    let parsed = Url::parse(trimmed).map_err(|source| EndpointError::Invalid {
        url: endpoint.to_owned(),
        source,
    })?;

    let scheme = match parsed.scheme() {
        "http" => "http",
        "https" => "https",
        other => return Err(EndpointError::UnsupportedScheme(other.to_owned())),
    };

    // Keep the original textual form (minus trailing slashes) instead of
    // `Url::to_string()`, which would add a `/` for a bare origin. IDN
    // conversion happens once the path has been appended, matching Go.
    Ok((trimmed.to_owned(), scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_becomes_ws_and_keeps_the_token() {
        assert_eq!(
            build_ws_endpoint("http://panel.example.com", "abc").unwrap(),
            "ws://panel.example.com/api/clients/report?token=abc"
        );
    }

    #[test]
    fn https_becomes_wss() {
        assert_eq!(
            build_ws_endpoint("https://panel.example.com", "tok").unwrap(),
            "wss://panel.example.com/api/clients/report?token=tok"
        );
    }

    #[test]
    fn trailing_slashes_and_path_prefixes_are_preserved_once() {
        assert_eq!(
            build_ws_endpoint("https://panel.example.com///", "t").unwrap(),
            "wss://panel.example.com/api/clients/report?token=t"
        );
        assert_eq!(
            build_ws_endpoint("https://panel.example.com/komari/", "t").unwrap(),
            "wss://panel.example.com/komari/api/clients/report?token=t"
        );
        assert_eq!(
            build_basic_info_endpoint("https://panel.example.com/", "t").unwrap(),
            "https://panel.example.com/api/clients/uploadBasicInfo?token=t"
        );
    }

    #[test]
    fn idn_hosts_are_converted_to_punycode() {
        assert_eq!(
            build_ws_endpoint("https://中文.example/", "t").unwrap(),
            "wss://xn--fiq228c.example/api/clients/report?token=t"
        );
        assert_eq!(
            build_basic_info_endpoint("http://中文.example", "t").unwrap(),
            "http://xn--fiq228c.example/api/clients/uploadBasicInfo?token=t"
        );
    }

    #[test]
    fn ipv4_ipv6_and_ports_are_kept_verbatim() {
        assert_eq!(
            build_ws_endpoint("https://1.1.1.1:8443", "t").unwrap(),
            "wss://1.1.1.1:8443/api/clients/report?token=t"
        );
        assert_eq!(
            build_ws_endpoint("http://[2001:db8::1]:8080", "t").unwrap(),
            "ws://[2001:db8::1]:8080/api/clients/report?token=t"
        );
    }

    #[test]
    fn invalid_endpoints_are_rejected() {
        assert!(matches!(
            build_ws_endpoint("", "t"),
            Err(EndpointError::Empty)
        ));
        assert!(matches!(
            build_ws_endpoint("ftp://panel.example.com", "t"),
            Err(EndpointError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            build_ws_endpoint("not a url", "t"),
            Err(EndpointError::Invalid { .. })
        ));
    }

    #[test]
    fn endpoint_without_scheme_is_rejected() {
        assert!(matches!(
            build_ws_endpoint("panel.example.com", "t"),
            Err(EndpointError::Invalid { .. })
        ));
    }
}
