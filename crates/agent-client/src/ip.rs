//! IP address selection for the basic information payload.
//!
//! `agent-monitor` owns every metric in the periodic report and the basic-info
//! `disk_total`/`gpu_name` fields, but it does not collect IP addresses. This
//! module is therefore the only A05-owned collector: it mirrors Go's
//! `monitoring.GetIPAddress` priority (NIC addresses, then custom values) while
//! intentionally skipping the external lookup services (D-020).
//!
//! On Unix interfaces are enumerated with `getifaddrs` and `IFF_UP`, matching
//! Go's `net.FlagUp`; `if-addrs::is_oper_up` is only used on non-Unix platforms
//! where it is an approximation (D-011).

use agent_monitor::NicFilter;

/// Options for IP address selection.
#[derive(Clone, Debug, Default)]
pub struct IpOptions {
    pub include_nics: String,
    pub exclude_nics: String,
    pub custom_ipv4: String,
    pub custom_ipv6: String,
    pub get_ip_addr_from_nic: bool,
}

/// Resolve IPv4/IPv6 addresses using Go's priority rules.
pub fn addresses(options: &IpOptions) -> (String, String) {
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
    )
}

/// Go's priority: NIC addresses (when enabled and found) win over custom
/// values; the external lookup service is intentionally not called.
fn select_addresses(
    custom_ipv4: &str,
    custom_ipv6: &str,
    nic_ipv4: String,
    nic_ipv6: String,
) -> (String, String) {
    if !nic_ipv4.is_empty() || !nic_ipv6.is_empty() {
        return (nic_ipv4, nic_ipv6);
    }
    (custom_ipv4.to_owned(), custom_ipv6.to_owned())
}

#[cfg(unix)]
fn nic_addresses(options: &IpOptions) -> (String, String) {
    use std::ffi::CStr;
    use std::net::{Ipv4Addr, Ipv6Addr};

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
        assert_eq!(
            addresses(&options),
            ("203.0.113.7".to_owned(), "2001:db8::7".to_owned())
        );
    }

    #[test]
    fn nic_addresses_win_over_custom_values_like_go() {
        assert_eq!(
            select_addresses("custom4", "custom6", "nic4".to_owned(), String::new()),
            ("nic4".to_owned(), String::new())
        );
        assert_eq!(
            select_addresses("custom4", "custom6", String::new(), String::new()),
            ("custom4".to_owned(), "custom6".to_owned())
        );
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
