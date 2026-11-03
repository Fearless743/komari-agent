//! End-to-end tests for the `komari-agent` binary.

use std::process::Command;

fn agent() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_komari-agent"));
    // Run with a clean environment: developer or CI variables such as
    // RUST_LOG, HOST_PROC or AGENT_* must not change test outcomes.
    command.env_clear();
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
    assert_eq!(
        stderr.matches("failed to read config file").count(),
        1,
        "the error must be printed exactly once: {stderr}"
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
    assert_eq!(
        stderr.matches("invalid --prefer-ip-version value").count(),
        1,
        "the error must be printed exactly once: {stderr}"
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
        .env("RUST_LOG", "info")
        .args(["--token", "secret", "--endpoint", "https://panel.example"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The token must never be printed in clear text.
    assert!(!stderr.contains("token: \"secret\""), "stderr: {stderr}");
    assert!(stderr.contains("******"));
}

#[test]
fn boolean_flags_do_not_consume_the_next_token() {
    let output = agent()
        .args(["--disable-auto-update", "foo"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("disable_auto_update: true"),
        "stderr: {stderr}"
    );

    let output = agent()
        .args(["-u", "foo"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ignore_unsafe_cert: true"),
        "stderr: {stderr}"
    );
}

#[test]
fn boolean_flags_support_the_equals_form() {
    let disabled = agent()
        .args(["--disable-auto-update=false"])
        .output()
        .expect("run komari-agent");
    assert_eq!(disabled.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&disabled.stderr).contains("disable_auto_update: false"),
        "stderr: {}",
        String::from_utf8_lossy(&disabled.stderr)
    );

    let enabled = agent()
        .arg("--disable-auto-update")
        .output()
        .expect("run komari-agent");
    assert_eq!(enabled.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&enabled.stderr).contains("disable_auto_update: true"),
        "stderr: {}",
        String::from_utf8_lossy(&enabled.stderr)
    );
}

#[test]
fn positional_arguments_are_ignored_like_go() {
    let output = agent()
        .args(["some-positional", "another", "--interval", "2.5"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn negative_numbers_are_accepted() {
    let output = agent()
        .args(["--interval", "-1", "--max-retries", "-1"])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(0));
}
