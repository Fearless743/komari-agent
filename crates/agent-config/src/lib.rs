//! Configuration loading for the Komari agent.
//!
//! Field names, JSON keys, environment variables and defaults mirror the Go
//! implementation (`komari-agent/cmd/flags/flag.go` and
//! `komari-agent/cmd/root.go`). Values are resolved with the following
//! precedence, from highest to lowest:
//!
//! 1. command line flags
//! 2. environment variables (`AGENT_*`, plus `HOST_PROC`)
//! 3. JSON config file (`AGENT_CONFIG_FILE` or `--config`)
//! 4. built-in defaults
//!
//! Secrets are never exposed through [`Config`]'s [`Debug`] implementation.

mod overrides;

pub use overrides::CliOverrides;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Environment variable that selects the JSON config file.
pub const CONFIG_FILE_ENV: &str = "AGENT_CONFIG_FILE";

/// Placeholder printed instead of secret values.
const MASK: &str = "******";

/// Resolved agent configuration.
///
/// The fields and defaults match the Go `Config` struct.
#[derive(Clone)]
pub struct Config {
    pub auto_discovery_key: String,
    pub disable_auto_update: bool,
    /// Deprecated, use [`Config::memory_include_cache`] instead.
    pub memory_mode_available: bool,
    pub token: String,
    pub endpoint: String,
    pub interval: f64,
    pub ignore_unsafe_cert: bool,
    pub max_retries: i32,
    pub reconnect_interval: i32,
    pub info_report_interval: i32,
    pub include_nics: String,
    pub exclude_nics: String,
    pub include_mountpoints: String,
    pub month_rotate: i32,
    pub cf_access_client_id: String,
    pub cf_access_client_secret: String,
    pub memory_include_cache: bool,
    pub memory_report_raw_used: bool,
    pub custom_dns: String,
    pub enable_gpu: bool,
    pub custom_ipv4: String,
    pub custom_ipv6: String,
    pub get_ip_addr_from_nic: bool,
    pub host_proc: String,
    pub config_file: String,
    pub protocol_version: i32,
    pub disable_compression: bool,
    pub prefer_ip_version: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_discovery_key: String::new(),
            disable_auto_update: false,
            memory_mode_available: false,
            token: String::new(),
            endpoint: String::new(),
            // Defaults below come from `cmd/root.go` flag registration.
            interval: 1.0,
            ignore_unsafe_cert: false,
            max_retries: 3,
            reconnect_interval: 5,
            info_report_interval: 5,
            include_nics: String::new(),
            exclude_nics: String::new(),
            include_mountpoints: String::new(),
            month_rotate: 0,
            cf_access_client_id: String::new(),
            cf_access_client_secret: String::new(),
            memory_include_cache: false,
            memory_report_raw_used: false,
            custom_dns: String::new(),
            enable_gpu: false,
            custom_ipv4: String::new(),
            custom_ipv6: String::new(),
            get_ip_addr_from_nic: false,
            host_proc: String::new(),
            config_file: String::new(),
            protocol_version: 2,
            disable_compression: false,
            prefer_ip_version: String::new(),
        }
    }
}

impl Config {
    /// Load the configuration from the process environment and command line.
    pub fn load(cli: CliOverrides) -> Result<Self, ConfigError> {
        Self::load_with_env(cli, &|key| std::env::var(key).ok())
    }

    /// Load the configuration using an injected environment lookup.
    ///
    /// The lookup is injectable so tests do not have to mutate the process
    /// environment. A non-empty value is treated as set; empty values are
    /// ignored, matching the Go loader.
    pub fn load_with_env(
        cli: CliOverrides,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        let config_path = non_empty(cli.config_file.as_deref())
            .map(PathBuf::from)
            .or_else(|| non_empty(env(CONFIG_FILE_ENV).as_deref()).map(PathBuf::from));

        if let Some(path) = &config_path {
            let file = read_config_file(path)?;
            config.apply_overrides(&file);
            config.config_file = path.to_string_lossy().into_owned();
        }

        config.apply_env(env);
        config.apply_overrides(&cli);
        config.normalize_and_validate()?;

        Ok(config)
    }

    fn apply_env(&mut self, env: &dyn Fn(&str) -> Option<String>) {
        macro_rules! string {
            ($field:ident, $key:expr) => {
                if let Some(value) = env($key).filter(|value| !value.is_empty()) {
                    self.$field = value;
                }
            };
        }

        // Go only enables booleans for "true" (case insensitive) and "1";
        // any other non-empty value explicitly turns the option off.
        macro_rules! boolean {
            ($field:ident, $key:expr) => {
                if let Some(value) = env($key).filter(|value| !value.is_empty()) {
                    self.$field = value.eq_ignore_ascii_case("true") || value == "1";
                }
            };
        }

        macro_rules! number {
            ($field:ident, $key:expr, $ty:ty) => {
                if let Some(value) = env($key).filter(|value| !value.is_empty())
                    && let Ok(parsed) = value.parse::<$ty>()
                {
                    self.$field = parsed;
                }
            };
        }

        string!(auto_discovery_key, "AGENT_AUTO_DISCOVERY_KEY");
        boolean!(disable_auto_update, "AGENT_DISABLE_AUTO_UPDATE");
        boolean!(memory_mode_available, "AGENT_MEMORY_MODE_AVAILABLE");
        string!(token, "AGENT_TOKEN");
        string!(endpoint, "AGENT_ENDPOINT");
        number!(interval, "AGENT_INTERVAL", f64);
        boolean!(ignore_unsafe_cert, "AGENT_IGNORE_UNSAFE_CERT");
        number!(max_retries, "AGENT_MAX_RETRIES", i32);
        number!(reconnect_interval, "AGENT_RECONNECT_INTERVAL", i32);
        number!(info_report_interval, "AGENT_INFO_REPORT_INTERVAL", i32);
        string!(include_nics, "AGENT_INCLUDE_NICS");
        string!(exclude_nics, "AGENT_EXCLUDE_NICS");
        string!(include_mountpoints, "AGENT_INCLUDE_MOUNTPOINTS");
        number!(month_rotate, "AGENT_MONTH_ROTATE", i32);
        string!(cf_access_client_id, "AGENT_CF_ACCESS_CLIENT_ID");
        string!(cf_access_client_secret, "AGENT_CF_ACCESS_CLIENT_SECRET");
        boolean!(memory_include_cache, "AGENT_MEMORY_INCLUDE_CACHE");
        boolean!(memory_report_raw_used, "AGENT_MEMORY_REPORT_RAW_USED");
        string!(custom_dns, "AGENT_CUSTOM_DNS");
        boolean!(enable_gpu, "AGENT_ENABLE_GPU");
        string!(custom_ipv4, "AGENT_CUSTOM_IPV4");
        string!(custom_ipv6, "AGENT_CUSTOM_IPV6");
        boolean!(get_ip_addr_from_nic, "AGENT_GET_IP_ADDR_FROM_NIC");
        string!(host_proc, "HOST_PROC");
        string!(config_file, CONFIG_FILE_ENV);
        number!(protocol_version, "AGENT_PROTOCOL_VERSION", i32);
        boolean!(disable_compression, "AGENT_DISABLE_COMPRESSION");
        string!(prefer_ip_version, "AGENT_PREFER_IP_VERSION");
    }

    fn apply_overrides(&mut self, overrides: &CliOverrides) {
        macro_rules! copy {
            ($field:ident) => {
                if let Some(value) = &overrides.$field {
                    self.$field = value.clone();
                }
            };
        }

        copy!(auto_discovery_key);
        copy!(disable_auto_update);
        copy!(memory_mode_available);
        copy!(token);
        copy!(endpoint);
        copy!(interval);
        copy!(ignore_unsafe_cert);
        copy!(max_retries);
        copy!(reconnect_interval);
        copy!(info_report_interval);
        copy!(include_nics);
        copy!(exclude_nics);
        copy!(include_mountpoints);
        copy!(month_rotate);
        copy!(cf_access_client_id);
        copy!(cf_access_client_secret);
        copy!(memory_include_cache);
        copy!(memory_report_raw_used);
        copy!(custom_dns);
        copy!(enable_gpu);
        copy!(custom_ipv4);
        copy!(custom_ipv6);
        copy!(get_ip_addr_from_nic);
        copy!(host_proc);
        copy!(config_file);
        copy!(protocol_version);
        copy!(disable_compression);
        copy!(prefer_ip_version);
    }

    fn normalize_and_validate(&mut self) -> Result<(), ConfigError> {
        // The Go agent falls back to protocol version 2 when it is unset.
        if self.protocol_version == 0 {
            self.protocol_version = 2;
        }

        if !self.prefer_ip_version.is_empty()
            && self.prefer_ip_version != "4"
            && self.prefer_ip_version != "6"
        {
            return Err(ConfigError::InvalidPreferIpVersion(
                self.prefer_ip_version.clone(),
            ));
        }

        Ok(())
    }

    /// Masked token, safe for logging.
    pub fn masked_token(&self) -> &str {
        mask(&self.token)
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("auto_discovery_key", &mask(&self.auto_discovery_key))
            .field("disable_auto_update", &self.disable_auto_update)
            .field("memory_mode_available", &self.memory_mode_available)
            .field("token", &mask(&self.token))
            .field("endpoint", &self.endpoint)
            .field("interval", &self.interval)
            .field("ignore_unsafe_cert", &self.ignore_unsafe_cert)
            .field("max_retries", &self.max_retries)
            .field("reconnect_interval", &self.reconnect_interval)
            .field("info_report_interval", &self.info_report_interval)
            .field("include_nics", &self.include_nics)
            .field("exclude_nics", &self.exclude_nics)
            .field("include_mountpoints", &self.include_mountpoints)
            .field("month_rotate", &self.month_rotate)
            .field("cf_access_client_id", &self.cf_access_client_id)
            .field(
                "cf_access_client_secret",
                &mask(&self.cf_access_client_secret),
            )
            .field("memory_include_cache", &self.memory_include_cache)
            .field("memory_report_raw_used", &self.memory_report_raw_used)
            .field("custom_dns", &self.custom_dns)
            .field("enable_gpu", &self.enable_gpu)
            .field("custom_ipv4", &self.custom_ipv4)
            .field("custom_ipv6", &self.custom_ipv6)
            .field("get_ip_addr_from_nic", &self.get_ip_addr_from_nic)
            .field("host_proc", &self.host_proc)
            .field("config_file", &self.config_file)
            .field("protocol_version", &self.protocol_version)
            .field("disable_compression", &self.disable_compression)
            .field("prefer_ip_version", &self.prefer_ip_version)
            .finish()
    }
}

/// Read a JSON config file into the same optional overlay type used for
/// command line flags. Omitted keys stay `None` and therefore never clobber a
/// value coming from a higher priority source.
fn read_config_file(path: &Path) -> Result<CliOverrides, ConfigError> {
    let bytes = fs::read(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    serde_json::from_slice(&bytes).map_err(|source| ConfigError::ParseFile {
        path: path.to_path_buf(),
        source,
    })
}

/// Configuration loading errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file {path}: {source}")]
    ParseFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid --prefer-ip-version value {0:?}: expected 4 or 6")]
    InvalidPreferIpVersion(String),
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn mask(value: &str) -> &str {
    if value.is_empty() { "" } else { MASK }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;

    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        move |key| values.get(key).cloned()
    }

    fn write_config(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("create temp config file");
        file.write_all(contents.as_bytes())
            .expect("write temp config file");
        file.flush().expect("flush temp config file");
        file
    }

    #[test]
    fn defaults_match_go_agent() {
        let config = Config::load_with_env(CliOverrides::default(), &no_env).unwrap();

        assert_eq!(config.token, "");
        assert_eq!(config.endpoint, "");
        assert_eq!(config.interval, 1.0);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.reconnect_interval, 5);
        assert_eq!(config.info_report_interval, 5);
        assert_eq!(config.month_rotate, 0);
        assert_eq!(config.protocol_version, 2);
        assert!(!config.ignore_unsafe_cert);
        assert!(!config.disable_auto_update);
        assert!(!config.disable_compression);
        assert_eq!(config.prefer_ip_version, "");
        assert_eq!(config.config_file, "");
    }

    #[test]
    fn environment_overrides_defaults() {
        let env = env_from(&[
            ("AGENT_TOKEN", "env-token"),
            ("AGENT_ENDPOINT", "https://env.example.com"),
            ("AGENT_INTERVAL", "2.5"),
            ("AGENT_IGNORE_UNSAFE_CERT", "1"),
            ("AGENT_MAX_RETRIES", "9"),
            ("AGENT_DISABLE_AUTO_UPDATE", "TRUE"),
            ("AGENT_PROTOCOL_VERSION", "1"),
            ("AGENT_INCLUDE_NICS", "eth0,eth1"),
            ("AGENT_PREFER_IP_VERSION", "6"),
            ("AGENT_CUSTOM_IPV4", "203.0.113.7"),
            ("HOST_PROC", "/host/proc"),
        ]);

        let config = Config::load_with_env(CliOverrides::default(), &env).unwrap();

        assert_eq!(config.token, "env-token");
        assert_eq!(config.endpoint, "https://env.example.com");
        assert_eq!(config.interval, 2.5);
        assert!(config.ignore_unsafe_cert);
        assert_eq!(config.max_retries, 9);
        assert!(config.disable_auto_update);
        assert_eq!(config.protocol_version, 1);
        assert_eq!(config.include_nics, "eth0,eth1");
        assert_eq!(config.prefer_ip_version, "6");
        assert_eq!(config.custom_ipv4, "203.0.113.7");
        assert_eq!(config.host_proc, "/host/proc");
    }

    #[test]
    fn config_file_is_loaded_and_environment_wins() {
        let file = write_config(
            r#"{
                "token": "file-token",
                "endpoint": "https://file.example.com",
                "interval": 10.0,
                "max_retries": 5,
                "memory_include_cache": true,
                "custom_ipv6": "2001:db8::1",
                "protocol_version": 1
            }"#,
        );

        let cli = CliOverrides {
            config_file: Some(file.path().to_string_lossy().into_owned()),
            ..CliOverrides::default()
        };

        let env = env_from(&[
            ("AGENT_INTERVAL", "3.0"),
            ("AGENT_TOKEN", "env-token"),
            ("AGENT_PROTOCOL_VERSION", "2"),
        ]);

        let config = Config::load_with_env(cli, &env).unwrap();

        assert_eq!(config.token, "env-token", "environment wins over file");
        assert_eq!(config.interval, 3.0, "environment wins over file");
        assert_eq!(config.protocol_version, 2);
        assert_eq!(config.endpoint, "https://file.example.com");
        assert_eq!(config.max_retries, 5);
        assert!(config.memory_include_cache);
        assert_eq!(config.custom_ipv6, "2001:db8::1");
        assert_eq!(config.config_file, file.path().to_string_lossy());
    }

    #[test]
    fn command_line_flags_win_over_environment_and_file() {
        let file = write_config(r#"{"token": "file-token", "interval": 10.0}"#);

        let cli = CliOverrides {
            config_file: Some(file.path().to_string_lossy().into_owned()),
            token: Some("cli-token".to_owned()),
            interval: Some(4.5),
            ..CliOverrides::default()
        };

        let env = env_from(&[("AGENT_TOKEN", "env-token"), ("AGENT_INTERVAL", "3.0")]);
        let config = Config::load_with_env(cli, &env).unwrap();

        assert_eq!(config.token, "cli-token");
        assert_eq!(config.interval, 4.5);
    }

    #[test]
    fn config_file_path_from_environment_is_used() {
        let file = write_config(r#"{"endpoint": "https://from-env-path.example.com"}"#);
        let path = file.path().to_string_lossy().into_owned();
        let env_pairs = [("AGENT_CONFIG_FILE", path.as_str())];
        let env = env_from(&env_pairs);

        let config = Config::load_with_env(CliOverrides::default(), &env).unwrap();

        assert_eq!(config.endpoint, "https://from-env-path.example.com");
        assert_eq!(config.config_file, file.path().to_string_lossy());
    }

    #[test]
    fn missing_config_file_reports_a_clear_error() {
        let cli = CliOverrides {
            config_file: Some("/nonexistent/komari-agent/config.json".to_owned()),
            ..CliOverrides::default()
        };

        let error = Config::load_with_env(cli, &no_env).unwrap_err();
        let rendered = error.to_string();

        assert!(matches!(error, ConfigError::ReadFile { .. }));
        assert!(rendered.contains("failed to read config file"));
        assert!(rendered.contains("config.json"));
    }

    #[test]
    fn invalid_config_file_json_is_rejected() {
        let file = write_config("{ not valid json");
        let cli = CliOverrides {
            config_file: Some(file.path().to_string_lossy().into_owned()),
            ..CliOverrides::default()
        };

        let error = Config::load_with_env(cli, &no_env).unwrap_err();

        assert!(matches!(error, ConfigError::ParseFile { .. }));
        assert!(error.to_string().contains("failed to parse config file"));
    }

    #[test]
    fn prefer_ip_version_is_validated() {
        let cli = CliOverrides {
            prefer_ip_version: Some("5".to_owned()),
            ..CliOverrides::default()
        };

        let error = Config::load_with_env(cli, &no_env).unwrap_err();

        assert!(matches!(error, ConfigError::InvalidPreferIpVersion(_)));
    }

    #[test]
    fn debug_output_masks_secrets() {
        let cli = CliOverrides {
            token: Some("super-secret-token".to_owned()),
            cf_access_client_secret: Some("cf-secret".to_owned()),
            auto_discovery_key: Some("discovery-secret".to_owned()),
            ..CliOverrides::default()
        };

        let config = Config::load_with_env(cli, &no_env).unwrap();
        let rendered = format!("{config:?}");

        assert!(!rendered.contains("super-secret-token"));
        assert!(!rendered.contains("cf-secret"));
        assert!(!rendered.contains("discovery-secret"));
        assert_eq!(rendered.matches(MASK).count(), 3);
    }
}
