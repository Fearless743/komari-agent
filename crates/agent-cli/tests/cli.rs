//! End-to-end tests for the `komari-agent` binary.
//!
//! Every test disables automatic updates and points at a closed local port, so
//! no test can reach a real panel or the GitHub release API.

use std::process::Command;

fn agent() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_komari-agent"));
    // Run with a clean environment: developer or CI variables such as
    // RUST_LOG, HOST_PROC or AGENT_* must not change test outcomes.
    command.env_clear();
    command
}

fn offline_args<'a>(args: &'a [&'a str]) -> Vec<&'a str> {
    let mut result = vec![
        "--token",
        "secret",
        "--endpoint",
        "http://127.0.0.1:1",
        "--max-retries",
        "0",
        "--reconnect-interval",
        "0",
        "--interval",
        "0.1",
    ];
    result.extend_from_slice(args);
    result
}

fn offline_args_with_update<'a>(args: &'a [&'a str]) -> Vec<&'a str> {
    let mut result = offline_args(&["--disable-auto-update"]);
    result.extend_from_slice(args);
    result
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
fn missing_credentials_are_reported_before_connecting() {
    let output = agent()
        .env("RUST_LOG", "off")
        .arg("--disable-auto-update")
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing credentials"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
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
fn retry_exhaustion_exits_with_failure() {
    let args = offline_args_with_update(&[]);
    let output = agent()
        .env("RUST_LOG", "info")
        .args(args)
        .output()
        .expect("run komari-agent");

    // Non-zero so systemd Restart=on-failure starts the agent again.
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The token must never be printed in clear text.
    assert!(!stderr.contains("token: \"secret\""), "stderr: {stderr}");
    assert!(stderr.contains("******"));
    assert!(stderr.contains("Max retries reached"), "stderr: {stderr}");
}

#[test]
fn boolean_flags_do_not_consume_the_next_token() {
    let args = offline_args_with_update(&["foo"]);
    let output = agent().args(args).output().expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("disable_auto_update: true"),
        "stderr: {stderr}"
    );

    let args = offline_args_with_update(&["-u", "foo"]);
    let output = agent().args(args).output().expect("run komari-agent");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ignore_unsafe_cert: true"),
        "stderr: {stderr}"
    );
}

#[test]
fn boolean_flags_support_the_equals_form() {
    // `--disable-auto-update=false` would start a real update check, so the
    // parsing assertion lives in the binary unit tests; here we only exercise
    // the offline enabled form end to end.
    let enabled = agent()
        .args(offline_args_with_update(&[]))
        .output()
        .expect("run komari-agent");
    assert_eq!(enabled.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&enabled.stderr).contains("disable_auto_update: true"),
        "stderr: {}",
        String::from_utf8_lossy(&enabled.stderr)
    );
}

#[test]
fn positional_arguments_are_ignored_like_go() {
    let args = offline_args_with_update(&["some-positional", "another"]);
    let output = agent().args(args).output().expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn negative_numbers_are_accepted() {
    let output = agent()
        .args([
            "--token",
            "secret",
            "--endpoint",
            "http://127.0.0.1:1",
            "--disable-auto-update",
            "--reconnect-interval",
            "0",
            "--interval",
            "-1",
            "--max-retries",
            "-1",
        ])
        .output()
        .expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn month_rotate_flag_is_accepted() {
    let args = offline_args_with_update(&["--month-rotate", "5"]);
    let output = agent().args(args).output().expect("run komari-agent");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("month_rotate: 5"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_mem_prints_the_go_sections() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("meminfo"),
        "MemTotal: 4096 kB\nMemFree: 1024 kB\nMemAvailable: 2048 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
    )
    .unwrap();

    let output = agent()
        .env("HOST_PROC", directory.path())
        .arg("check-mem")
        .output()
        .expect("run check-mem");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--- Memory Check ---"), "{stdout}");
    assert!(stdout.contains("MemTotal: 4 MiB"), "{stdout}");
    assert!(stdout.contains("[callFree]"), "{stdout}");
    assert!(stdout.contains("--- Current Configured ---"), "{stdout}");
}

#[test]
fn list_disk_prints_a_partition_table() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("mounts"),
        "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /data xfs rw 0 0\n",
    )
    .unwrap();

    let output = agent()
        .env("HOST_PROC", directory.path())
        .args(["list-disk", "--include-mountpoint", "/;/data"])
        .output()
        .expect("run list-disk");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("All Disk Partitions:"), "{stdout}");
    assert!(stdout.contains("Mountpoint\tFstype"), "{stdout}");
    assert!(stdout.contains("/data\txfs"), "{stdout}");
    assert!(
        stdout.contains("Monitoring Mountpoints: [/ /data]"),
        "{stdout}"
    );
}
