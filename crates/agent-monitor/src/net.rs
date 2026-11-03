//! Network traffic collection.
//!
//! Mirrors `monitoring/unit/net.go` and `monitoring/netstatic/static.go`:
//!
//! * `include_nics` / `exclude_nics` are comma separated and support Go
//!   `filepath.Match` wildcards;
//! * well-known virtual/loopback prefixes are always excluded;
//! * current rates are the difference between two samples one interval apart;
//! * totals are cumulative interface byte counters, or the sum since the last
//!   monthly reset when `month_rotate` is set;
//! * counters that wrap or reset clamp to zero.

use std::collections::BTreeSet;

use globset::{Glob, GlobMatcher};
use parking_lot::Mutex;

use crate::netstatic::{NetStaticStore, current_reset_timestamp};
use crate::options::MonitorOptions;
use crate::types::{InterfaceCounters, NetworkReport, NetworkSpeed};

/// Interface name prefixes excluded unconditionally, matching Go's
/// `loopbackNames` map.
const EXCLUDED_PREFIXES: [&str; 12] = [
    "br", "cni", "docker", "podman", "flannel", "lo", "veth", "virbr", "vmbr", "tap", "fwbr",
    "fwpr",
];

/// Parsed interface filters.
#[derive(Debug, Clone, Default)]
pub struct NicFilter {
    include: Vec<GlobMatcher>,
    exclude: Vec<GlobMatcher>,
    has_include: bool,
}

impl NicFilter {
    /// Parse the comma separated Go-agent filters.
    pub fn new(include_nics: &str, exclude_nics: &str) -> Self {
        let include = parse_nics(include_nics);
        let exclude = parse_nics(exclude_nics);
        Self {
            include: compile_patterns(&include),
            exclude: compile_patterns(&exclude),
            has_include: !include.is_empty(),
        }
    }

    /// Go's `shouldInclude`.
    pub fn should_include(&self, nic_name: &str) -> bool {
        if EXCLUDED_PREFIXES
            .iter()
            .any(|prefix| nic_name.starts_with(prefix))
        {
            return false;
        }

        if self
            .include
            .iter()
            .any(|pattern| pattern.is_match(nic_name))
        {
            return true;
        }
        if self
            .exclude
            .iter()
            .any(|pattern| pattern.is_match(nic_name))
        {
            return false;
        }

        !self.has_include
    }
}

fn compile_patterns(patterns: &[String]) -> Vec<GlobMatcher> {
    patterns
        .iter()
        .filter_map(|pattern| Glob::new(pattern).ok())
        .map(|glob| glob.compile_matcher())
        .collect()
}

/// Parse `include_nics` / `exclude_nics`: comma separated, trimmed, deduped.
///
/// An empty string yields an empty set (Go's `nil` map); a string containing
/// only separators yields non-empty patterns and therefore excludes everything
/// by default, matching the Go map semantics.
pub fn parse_nics(nics: &str) -> Vec<String> {
    if nics.is_empty() {
        return Vec::new();
    }
    nics.split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Convenience wrapper mirroring Go's `shouldInclude`.
pub fn should_include(nic_name: &str, include_nics: &str, exclude_nics: &str) -> bool {
    NicFilter::new(include_nics, exclude_nics).should_include(nic_name)
}

/// Parse `/proc/net/dev`.
pub fn parse_proc_net_dev(content: &str) -> Vec<InterfaceCounters> {
    content
        .lines()
        .filter_map(|line| {
            let (name, values) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name.contains('|') {
                return None;
            }
            let fields: Vec<u64> = values
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            if fields.len() < 10 {
                return None;
            }
            Some(InterfaceCounters {
                name: name.to_owned(),
                rx_bytes: fields[0],
                rx_packets: fields[1],
                tx_bytes: fields[8],
                tx_packets: fields[9],
            })
        })
        .collect()
}

/// Compute totals and rates from two samples.
pub fn network_speed(
    first: &[InterfaceCounters],
    second: &[InterfaceCounters],
    filter: &NicFilter,
) -> NetworkSpeed {
    let total_first = sum_selected(first, filter);
    let total_second = sum_selected(second, filter);

    NetworkSpeed {
        up: total_second.tx.saturating_sub(total_first.tx),
        down: total_second.rx.saturating_sub(total_first.rx),
        total_up: total_second.tx,
        total_down: total_second.rx,
    }
}

fn sum_selected(counters: &[InterfaceCounters], filter: &NicFilter) -> Totals {
    counters
        .iter()
        .filter(|counter| filter.should_include(&counter.name))
        .fold(Totals::default(), |mut totals, counter| {
            totals.tx = totals.tx.saturating_add(counter.tx_bytes);
            totals.rx = totals.rx.saturating_add(counter.rx_bytes);
            totals
        })
}

#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    tx: u64,
    rx: u64,
}

/// Read cumulative interface counters.
pub(crate) fn read_counters(options: &MonitorOptions) -> Result<Vec<InterfaceCounters>, String> {
    #[cfg(not(target_os = "linux"))]
    let _ = options;

    #[cfg(target_os = "linux")]
    {
        let path = options.proc_path("net/dev");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let counters = parse_proc_net_dev(&content);
                if counters.is_empty() {
                    return Err(format!("no network interfaces found in {}", path.display()));
                }
                return Ok(counters);
            }
            Err(error) => {
                if options.strict_proc {
                    return Err(format!("failed to read {}: {error}", path.display()));
                }
            }
        }
    }

    let counters = crate::platform::fallback::network_counters();
    if counters.is_empty() {
        return Err("no network interfaces found".to_owned());
    }
    Ok(counters)
}

/// Collect network metrics.
///
/// Returns the report and an optional diagnostic message (mirroring Go's
/// accumulated `message`). On failure the report contains zero rates; the
/// monthly totals are preserved when only the live sample failed.
pub(crate) fn collect(
    options: &MonitorOptions,
    state: &Mutex<Option<NetStaticStore>>,
) -> (NetworkReport, Option<String>) {
    let filter = NicFilter::new(&options.include_nics, &options.exclude_nics);

    let first = match read_counters(options) {
        Ok(counters) => counters,
        Err(error) => {
            return (
                NetworkReport::default(),
                Some(format!("failed to get network speed: {error}\n")),
            );
        }
    };

    if options.month_rotate != 0 {
        return collect_monthly(options, state, &filter, first);
    }

    std::thread::sleep(options.network_sample_interval);
    let second = match read_counters(options) {
        Ok(counters) => counters,
        Err(error) => {
            return (
                NetworkReport::default(),
                Some(format!("failed to get network speed: {error}\n")),
            );
        }
    };

    (network_speed(&first, &second, &filter).into(), None)
}

fn collect_monthly(
    options: &MonitorOptions,
    state: &Mutex<Option<NetStaticStore>>,
    filter: &NicFilter,
    first: Vec<InterfaceCounters>,
) -> (NetworkReport, Option<String>) {
    let now = current_unix_timestamp();
    let reset = current_reset_timestamp(options.month_rotate);
    let counters: Vec<(String, u64, u64)> = first
        .iter()
        .map(|counter| (counter.name.clone(), counter.tx_bytes, counter.rx_bytes))
        .collect();

    let mut guard = state.lock();
    let store = guard.get_or_insert_with(|| NetStaticStore::load(&options.net_static_path));
    let flush_due = store.record_filtered(&counters, now, |name| filter.should_include(name));
    if flush_due && let Err(error) = store.save() {
        tracing::warn!(
            path = %store.path().display(),
            %error,
            "failed to save monthly network traffic database"
        );
    }
    let totals = store.totals_between(reset, now);
    drop(guard);

    let mut total_up = 0u64;
    let mut total_down = 0u64;
    for (name, traffic) in &totals {
        if filter.should_include(name) {
            total_up = total_up.saturating_add(traffic.tx);
            total_down = total_down.saturating_add(traffic.rx);
        }
    }

    std::thread::sleep(options.network_sample_interval);
    let second = match read_counters(options) {
        Ok(counters) => counters,
        Err(error) => {
            return (
                NetworkReport {
                    up: 0,
                    down: 0,
                    total_up,
                    total_down,
                },
                Some(format!("failed to get network speed: {error}\n")),
            );
        }
    };
    let live = network_speed(&first, &second, filter);

    (
        NetworkReport {
            up: live.up,
            down: live.down,
            total_up,
            total_down,
        },
        None,
    )
}

fn current_unix_timestamp() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROC_NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0
  eth0: 5000 50 0 0 0 0 0 0 4000 40 0 0 0 0 0 0
 wlan0: 7000 70 0 0 0 0 0 0 6000 60 0 0 0 0 0 0
docker0: 9000 90 0 0 0 0 0 0 8000 80 0 0 0 0 0 0
";

    fn counters(name: &str, rx: u64, tx: u64) -> InterfaceCounters {
        InterfaceCounters {
            name: name.to_owned(),
            rx_bytes: rx,
            rx_packets: 0,
            tx_bytes: tx,
            tx_packets: 0,
        }
    }

    #[test]
    fn filters_loopback_and_virtual_interfaces_like_go() {
        assert!(!should_include("lo", "", ""));
        assert!(!should_include("docker0", "", ""));
        assert!(!should_include("veth123", "", ""));
        assert!(!should_include("vmbr0", "", ""));
        assert!(should_include("eth0", "", ""));
        assert!(should_include("enp3s0", "", ""));

        // Loopback wins even when explicitly included.
        assert!(!should_include("lo", "lo", ""));
        assert!(should_include("eth0", "eth*", ""));
        assert!(!should_include("wlan0", "eth*", ""));
        assert!(!should_include("tun0", "", "tun*"));
        assert!(should_include("wlan0", "", "eth*"));
        // Go checks the include list first: an explicit include wins over an
        // exclude pattern.
        assert!(should_include("eth0", "eth0", "eth*"));
        assert!(!should_include("wlan0", "eth0", "eth*"));
    }

    #[test]
    fn parses_proc_net_dev_counters() {
        let counters = parse_proc_net_dev(PROC_NET_DEV);
        assert_eq!(counters.len(), 4);
        assert_eq!(counters[1].name, "eth0");
        assert_eq!(counters[1].rx_bytes, 5000);
        assert_eq!(counters[1].rx_packets, 50);
        assert_eq!(counters[1].tx_bytes, 4000);
        assert_eq!(counters[1].tx_packets, 40);
    }

    #[test]
    fn computes_rates_and_cumulative_totals() {
        let first = vec![
            counters("eth0", 1000, 2000),
            counters("lo", 10, 20),
            counters("wlan0", 500, 600),
        ];
        let second = vec![
            counters("eth0", 1600, 2900),
            counters("lo", 20, 30),
            counters("wlan0", 700, 900),
        ];
        let filter = NicFilter::new("", "");
        let speed = network_speed(&first, &second, &filter);
        assert_eq!(speed.down, 800);
        assert_eq!(speed.up, 1200);
        assert_eq!(speed.total_down, 2300);
        assert_eq!(speed.total_up, 3800);

        let only_eth = network_speed(&first, &second, &NicFilter::new("eth*", ""));
        assert_eq!(only_eth.down, 600);
        assert_eq!(only_eth.up, 900);
        assert_eq!(only_eth.total_down, 1600);
        assert_eq!(only_eth.total_up, 2900);
    }

    #[test]
    fn counter_wrap_and_interface_removal_clamp_to_zero() {
        // Deliberate difference (D-016): Go's uint64 subtraction would report
        // a near-2^64 rate here; the task requires wrap protection.
        let first = vec![counters("eth0", 9000, 8000)];
        let second = vec![counters("eth0", 10, 20)];
        let speed = network_speed(&first, &second, &NicFilter::new("", ""));
        assert_eq!(speed.down, 0);
        assert_eq!(speed.up, 0);
        assert_eq!(speed.total_down, 10);
        assert_eq!(speed.total_up, 20);
    }

    #[test]
    fn empty_nic_strings_match_go_map_semantics() {
        assert!(parse_nics("").is_empty());
        assert_eq!(parse_nics(" eth0 , eth0 ,wlan0 "), vec!["eth0", "wlan0"]);
        // Only separators: a non-empty pattern set that matches nothing.
        let filter = NicFilter::new(" , ", "");
        assert!(!filter.should_include("eth0"));
    }
}
