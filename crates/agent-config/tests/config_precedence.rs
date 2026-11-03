//! End-to-end tests for configuration resolution.

use std::io::Write;

use agent_config::{CliOverrides, Config};
use clap::Parser;

/// Mirrors how `agent-cli` embeds the overrides.
#[derive(Parser)]
struct TestCli {
    #[command(flatten)]
    overrides: CliOverrides,
}

fn write_config(contents: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create temp config file");
    file.write_all(contents.as_bytes())
        .expect("write temp config file");
    file.flush().expect("flush temp config file");
    file
}

fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let values: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();

    move |key| values.get(key).cloned()
}

#[test]
fn precedence_is_flags_then_env_then_file_then_defaults() {
    let file = write_config(
        r#"{
            "token": "file-token",
            "endpoint": "https://file.example.com",
            "interval": 30.0,
            "protocol_version": 1,
            "custom_dns": "1.1.1.1"
        }"#,
    );

    let cli = CliOverrides {
        config_file: Some(file.path().to_string_lossy().into_owned()),
        interval: Some(2.5),
        ..CliOverrides::default()
    };

    let env = env_from(&[
        ("AGENT_TOKEN", "env-token"),
        ("AGENT_INTERVAL", "10.0"),
        ("AGENT_PROTOCOL_VERSION", "2"),
        ("AGENT_CUSTOM_DNS", "8.8.8.8"),
    ]);

    let config = Config::load_with_env(cli, &env).expect("config should load");

    // Flag beats environment and file.
    assert_eq!(config.interval, 2.5);
    // Environment beats file.
    assert_eq!(config.token, "env-token");
    assert_eq!(config.protocol_version, 2);
    assert_eq!(config.custom_dns, "8.8.8.8");
    // File beats default.
    assert_eq!(config.endpoint, "https://file.example.com");
    // Untouched default.
    assert_eq!(config.max_retries, 3);
}

#[test]
fn boolean_environment_values_only_enable_on_true_or_one() {
    let env = env_from(&[
        ("AGENT_MEMORY_INCLUDE_CACHE", "TRUE"),
        ("AGENT_GET_IP_ADDR_FROM_NIC", "1"),
        ("AGENT_ENABLE_GPU", "false"),
        ("AGENT_DISABLE_COMPRESSION", "yes"),
    ]);

    let config = Config::load_with_env(CliOverrides::default(), &env).expect("config should load");

    assert!(config.memory_include_cache);
    assert!(config.get_ip_addr_from_nic);
    assert!(
        !config.enable_gpu,
        "only \"true\" and \"1\" enable a boolean"
    );
    assert!(
        !config.disable_compression,
        "other values leave the previous value untouched"
    );
}

#[test]
fn boolean_environment_values_do_not_disable_file_values() {
    let file = write_config(r#"{"ignore_unsafe_cert": true, "memory_include_cache": true}"#);
    let cli = CliOverrides {
        config_file: Some(file.path().to_string_lossy().into_owned()),
        ..CliOverrides::default()
    };

    for value in ["0", "false", "yes", "no", "off", "TRUE "] {
        let env = env_from(&[
            ("AGENT_IGNORE_UNSAFE_CERT", value),
            ("AGENT_MEMORY_INCLUDE_CACHE", value),
        ]);
        let config = Config::load_with_env(cli.clone(), &env).expect("config should load");

        assert!(
            config.ignore_unsafe_cert,
            "env value {value:?} must not disable a config file value"
        );
        assert!(
            config.memory_include_cache,
            "env value {value:?} must not disable a config file value"
        );
    }
}

#[test]
fn invalid_numbers_do_not_abort_loading() {
    // The Go loader silently keeps the previous value when parsing fails.
    let env = env_from(&[
        ("AGENT_MAX_RETRIES", "not-a-number"),
        ("AGENT_INTERVAL", "NaN"),
    ]);

    let config = Config::load_with_env(CliOverrides::default(), &env).expect("config should load");

    assert_eq!(config.max_retries, 3);
    assert!(config.interval.is_nan());
}

#[test]
fn command_line_flags_are_parsed_with_go_names() {
    let cli = TestCli::try_parse_from([
        "komari-agent",
        "--token",
        "cli-token",
        "-e",
        "https://panel.example.com",
        "-i",
        "2.5",
        "-r",
        "7",
        "-c",
        "9",
        "--ignore-unsafe-cert",
        "--include-mountpoint",
        "/;/data",
        "--protocol-version",
        "1",
        "--prefer-ip-version",
        "4",
    ])
    .expect("flags should parse")
    .overrides;

    assert_eq!(cli.token.as_deref(), Some("cli-token"));
    assert_eq!(cli.endpoint.as_deref(), Some("https://panel.example.com"));
    assert_eq!(cli.interval, Some(2.5));
    assert_eq!(cli.max_retries, Some(7));
    assert_eq!(cli.reconnect_interval, Some(9));
    assert_eq!(cli.ignore_unsafe_cert, Some(true));
    assert_eq!(cli.include_mountpoints.as_deref(), Some("/;/data"));
    assert_eq!(cli.protocol_version, Some(1));
    assert_eq!(cli.prefer_ip_version.as_deref(), Some("4"));
    assert_eq!(cli.max_retries, Some(7));
}

#[test]
fn omitted_flags_stay_none() {
    let cli = TestCli::try_parse_from(["komari-agent"])
        .expect("no flags should parse")
        .overrides;

    assert_eq!(cli.token, None);
    assert_eq!(cli.interval, None);
    assert_eq!(cli.ignore_unsafe_cert, None);
    assert_eq!(cli.config_file, None);
}

#[test]
fn secret_values_never_reach_debug_output() {
    let cli = CliOverrides {
        token: Some("token-value".to_owned()),
        cf_access_client_secret: Some("cf-secret-value".to_owned()),
        ..CliOverrides::default()
    };

    let config = Config::load_with_env(cli, &|_| None).expect("config should load");
    let rendered = format!("{config:?}");

    assert!(!rendered.contains("token-value"));
    assert!(!rendered.contains("cf-secret-value"));
    assert_eq!(config.masked_token(), "******");
}
