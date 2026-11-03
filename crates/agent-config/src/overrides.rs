//! Command line overrides for the agent configuration.
//!
//! Every field is optional: `None` means "not provided on the command line",
//! which lets the loader apply the documented precedence
//! (flags > environment > config file > defaults).

use serde::Deserialize;

/// Parsed command line values that can override the configuration.
///
/// The same type is reused for JSON config file parsing because both layers
/// need the exact same "only set what is present" semantics. Fields that do
/// not have a command line flag in the Go agent are skipped by clap.
#[derive(Clone, Default, Deserialize, clap::Args)]
pub struct CliOverrides {
    #[arg(
        long = "auto-discovery",
        value_name = "KEY",
        help = "Auto discovery key for the agent"
    )]
    pub auto_discovery_key: Option<String>,

    #[arg(
        long = "disable-auto-update",
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Disable automatic updates"
    )]
    pub disable_auto_update: Option<bool>,

    /// Deprecated in the Go agent; kept for environment / config file parity.
    #[arg(skip)]
    pub memory_mode_available: Option<bool>,

    #[arg(long = "token", short = 't', value_name = "TOKEN", help = "API token")]
    pub token: Option<String>,

    #[arg(
        long = "endpoint",
        short = 'e',
        value_name = "ENDPOINT",
        help = "API endpoint"
    )]
    pub endpoint: Option<String>,

    #[arg(
        long = "interval",
        allow_negative_numbers = true,
        short = 'i',
        value_name = "SECONDS",
        help = "Interval in seconds"
    )]
    pub interval: Option<f64>,

    #[arg(
        long = "ignore-unsafe-cert",
        short = 'u',
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Ignore unsafe certificate errors"
    )]
    pub ignore_unsafe_cert: Option<bool>,

    #[arg(
        long = "max-retries",
        allow_negative_numbers = true,
        short = 'r',
        value_name = "COUNT",
        help = "Maximum number of retries"
    )]
    pub max_retries: Option<i64>,

    #[arg(
        long = "reconnect-interval",
        allow_negative_numbers = true,
        short = 'c',
        value_name = "SECONDS",
        help = "Reconnect interval in seconds"
    )]
    pub reconnect_interval: Option<i64>,

    #[arg(
        long = "info-report-interval",
        allow_negative_numbers = true,
        value_name = "MINUTES",
        help = "Interval in minutes for reporting basic info"
    )]
    pub info_report_interval: Option<i64>,

    #[arg(
        long = "include-nics",
        value_name = "NICS",
        help = "Comma-separated list of network interfaces to include"
    )]
    pub include_nics: Option<String>,

    #[arg(
        long = "exclude-nics",
        value_name = "NICS",
        help = "Comma-separated list of network interfaces to exclude"
    )]
    pub exclude_nics: Option<String>,

    #[arg(
        long = "include-mountpoint",
        value_name = "MOUNTPOINTS",
        help = "Semicolon-separated list of mount points to include for disk statistics"
    )]
    pub include_mountpoints: Option<String>,

    #[arg(
        long = "month-rotate",
        allow_negative_numbers = true,
        value_name = "DAY",
        help = "Month reset for network statistics (0 to disable)"
    )]
    pub month_rotate: Option<i64>,

    #[arg(
        long = "cf-access-client-id",
        value_name = "ID",
        help = "Cloudflare Access Client ID"
    )]
    pub cf_access_client_id: Option<String>,

    #[arg(
        long = "cf-access-client-secret",
        value_name = "SECRET",
        help = "Cloudflare Access Client Secret"
    )]
    pub cf_access_client_secret: Option<String>,

    #[arg(
        long = "memory-include-cache",
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Include cache/buffer in memory usage"
    )]
    pub memory_include_cache: Option<bool>,

    #[arg(
        long = "memory-exclude-bcf",
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Use \"raminfo.Used = v.Total - v.Free - v.Buffers - v.Cached\" calculation for memory usage"
    )]
    pub memory_report_raw_used: Option<bool>,

    #[arg(
        long = "custom-dns",
        value_name = "SERVER",
        help = "Custom DNS server to use (e.g. 8.8.8.8, 114.114.114.114). By default, the program uses the system DNS resolver."
    )]
    pub custom_dns: Option<String>,

    #[arg(
        long = "gpu",
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Enable detailed GPU monitoring (usage, memory, multi-GPU support)"
    )]
    pub enable_gpu: Option<bool>,

    #[arg(
        long = "custom-ipv4",
        value_name = "ADDR",
        help = "Custom IPv4 address to use"
    )]
    pub custom_ipv4: Option<String>,

    #[arg(
        long = "custom-ipv6",
        value_name = "ADDR",
        help = "Custom IPv6 address to use"
    )]
    pub custom_ipv6: Option<String>,

    #[arg(
        long = "get-ip-addr-from-nic",
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Get IP address from network interface"
    )]
    pub get_ip_addr_from_nic: Option<bool>,

    /// The Go agent only exposes this through `HOST_PROC` and the JSON file.
    #[arg(skip)]
    pub host_proc: Option<String>,

    #[arg(
        long = "config",
        value_name = "FILE",
        help = "Path to the configuration file"
    )]
    pub config_file: Option<String>,

    #[arg(
        long = "protocol-version",
        allow_negative_numbers = true,
        value_name = "VERSION",
        help = "Report protocol version (only 1 is supported; v2 was removed)"
    )]
    pub protocol_version: Option<i64>,

    #[arg(
        long = "disable-compression",
        num_args = 0..=1, require_equals = true,
        default_missing_value = "true",
        value_name = "BOOL",
        help = "Disable WebSocket permessage-deflate compression (v2 gzip is not implemented)"
    )]
    pub disable_compression: Option<bool>,

    #[arg(
        long = "prefer-ip-version",
        value_name = "VERSION",
        help = "Prefer IP version for dashboard connections: 4 or 6"
    )]
    pub prefer_ip_version: Option<String>,
}
