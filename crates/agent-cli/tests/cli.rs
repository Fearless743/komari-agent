//! End-to-end tests for the `komari-agent` binary.

use std::process::Command;

fn agent() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_komari-agent"));
    // Keep the tests independent from the developer's environment.
    command
        .env_remove("AGENT_CONFIG_FILE")
        .env_remove("AGENT_TOKEN")
        .env_remove("AGENT_ENDPOINT");
    command
}

#[test]
fn missing_config_file_prints_an_error_even_with_logging_disabled() {
    let output = agent()
        .env("RUST_LOG", "off")
        .args(["--config", "/nonexistent/komari-agent/a01-config.json"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to read config file"),
        "stderr did not contain the error: {stderr}"
    );
}

#[test]
fn invalid_prefer_ip_version_exits_with_an_error() {
    let output = agent()
        .env("RUST_LOG", "off")
        .args(["--prefer-ip-version", "5"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid --prefer-ip-version value"),
        "stderr did not contain the error: {stderr}"
    );
}

#[test]
fn help_and_version_exit_successfully() {
    let help = agent().arg("--help").output().expect("run --help");
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("komari-agent"));

    let version = agent().arg("--version").output().expect("run --version");
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).contains("komari-agent"));
}

#[test]
fn valid_flags_load_configuration_and_exit_successfully() {
    let output = agent()
        .args(["--token", "secret", "--endpoint", "https://panel.example"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The token must never be printed in clear text.
    assert!(!stderr.contains("token: \"secret\""), "stderr: {stderr}");
    assert!(stderr.contains("******"));
}
