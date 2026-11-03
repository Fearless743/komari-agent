//! Operating system information.
//!
//! Distribution detection follows the Go agent's order: Android, Proxmox VE,
//! Synology DSM, fnOS, then `os-release` PRETTY_NAME, then `Linux`.

#[cfg(target_os = "linux")]
use std::fs;

use crate::options::MonitorOptions;
use crate::platform;
use crate::types::OsInfo;

#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

/// Distribution names read by the Linux implementation, in lookup order.
const OS_RELEASE_PATHS: [&str; 2] = ["etc/os-release", "usr/lib/os-release"];

/// Collect distribution name, kernel version and architecture.
pub(crate) fn collect(options: &MonitorOptions) -> OsInfo {
    OsInfo {
        name: os_name(options),
        kernel_version: kernel_version(options),
        arch: arch(),
    }
}

/// Distribution name.
///
/// Linux uses the same special-purpose probes as Go's `OSName()` and falls
/// back to `PRETTY_NAME`/`Linux`. Other platforms use `sysinfo`.
pub(crate) fn os_name(options: &MonitorOptions) -> String {
    #[cfg(target_os = "linux")]
    {
        detect_android(options)
            .or_else(|| detect_proxmox(options))
            .or_else(|| detect_synology(options))
            .or_else(|| detect_fnos(options))
            .or_else(|| read_os_release(options))
            .unwrap_or_else(platform::fallback::os_name)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        platform::fallback::os_name()
    }
}

/// Kernel release.
///
/// On Linux this is read from the injectable proc root
/// (`sys/kernel/osrelease`), so a mounted host proc reports the host kernel.
/// `sysinfo` is used when the file is unavailable.
pub(crate) fn kernel_version(options: &MonitorOptions) -> String {
    #[cfg(not(target_os = "linux"))]
    let _ = options;

    #[cfg(target_os = "linux")]
    {
        if let Ok(value) = fs::read_to_string(options.proc_path("sys/kernel/osrelease")) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }

    platform::fallback::kernel_version()
}

/// Architecture string using Go's `GOARCH` naming, as uploaded by the Go agent.
pub(crate) fn arch() -> String {
    map_arch(std::env::consts::ARCH)
}

/// Map Rust's target architecture to the value Go reports through
/// `runtime.GOARCH`.
pub(crate) fn map_arch(arch: &str) -> String {
    match arch {
        "x86_64" => "amd64",
        "x86" => "386",
        "aarch64" => "arm64",
        "loongarch64" => "loong64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        "mips64el" => "mips64le",
        other => other,
    }
    .to_owned()
}

/// Android detection mirrors Go: `getprop` on a normal root, then
/// `/system/build.prop`, then the presence of typical Android directories.
#[cfg(target_os = "linux")]
fn detect_android(options: &MonitorOptions) -> Option<String> {
    if is_default_root(options) {
        if let Some(version) = getprop("ro.build.version.release") {
            let model = getprop("ro.product.model").unwrap_or_default();
            let brand = getprop("ro.product.brand").unwrap_or_default();
            return Some(format_android(&version, &model, &brand));
        }
    }

    if let Ok(content) = fs::read_to_string(options.root_path("system/build.prop"))
        && let Some(name) = parse_android_build_prop(&content)
    {
        return Some(name);
    }

    let android_dirs = ["system/app", "system/priv-app", "data/app", "sdcard"];
    let dir_count = android_dirs
        .iter()
        .filter(|relative| options.root_path(relative).is_dir())
        .count();
    if dir_count >= 2 {
        return Some("Android".to_owned());
    }

    None
}

#[cfg(target_os = "linux")]
fn getprop(key: &str) -> Option<String> {
    let output = Command::new("getprop").arg(key).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(target_os = "linux")]
fn format_android(version: &str, model: &str, brand: &str) -> String {
    let mut result = format!("Android {version}");
    if !model.is_empty() {
        if !brand.is_empty() && brand != model {
            result.push_str(&format!(" ({brand} {model})"));
        } else {
            result.push_str(&format!(" ({model})"));
        }
    }
    result
}

/// Parse `/system/build.prop` for the Android release, model and brand.
#[cfg(target_os = "linux")]
fn parse_android_build_prop(content: &str) -> Option<String> {
    let mut version = None;
    let mut model = None;
    let mut brand = None;

    for line in content.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "ro.build.version.release" => version = Some(value.trim().to_owned()),
            "ro.product.model" => model = Some(value.trim().to_owned()),
            "ro.product.brand" => brand = Some(value.trim().to_owned()),
            _ => {}
        }
    }

    let version = version.filter(|value| !value.is_empty())?;
    Some(format_android(
        &version,
        model.as_deref().unwrap_or_default(),
        brand.as_deref().unwrap_or_default(),
    ))
}

/// Proxmox VE detection mirrors Go's `pveversion` probe. The binary path is
/// resolved below the injected root, so tests can supply a fake executable.
#[cfg(target_os = "linux")]
fn detect_proxmox(options: &MonitorOptions) -> Option<String> {
    let output = proxmox_version_output(options)?;
    let version = parse_proxmox_version(&output);
    let codename = read_os_release_value(options, "VERSION_CODENAME");

    match (version, codename) {
        (Some(version), Some(codename)) if !codename.is_empty() => {
            Some(format!("Proxmox VE {version} ({codename})"))
        }
        (Some(version), _) => Some(format!("Proxmox VE {version}")),
        (None, _) => Some("Proxmox VE".to_owned()),
    }
}

#[cfg(target_os = "linux")]
fn proxmox_version_output(options: &MonitorOptions) -> Option<String> {
    let output = if is_default_root(options) {
        Command::new("pveversion").output().ok()?
    } else {
        let binary = options.root_path("usr/bin/pveversion");
        if !binary.is_file() {
            return None;
        }
        Command::new(binary).output().ok()?
    };

    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Extract `pve-manager/<version>~...` from `pveversion` output.
#[cfg(target_os = "linux")]
fn parse_proxmox_version(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pve-manager/") else {
            continue;
        };
        let version = rest.split('/').next().unwrap_or(rest);
        let version = version.split('~').next().unwrap_or(version).trim();
        if !version.is_empty() {
            return Some(version.to_owned());
        }
    }
    None
}

/// Synology DSM detection mirrors Go: `synoinfo.conf` in either location, then
/// `/usr/syno`.
#[cfg(target_os = "linux")]
fn detect_synology(options: &MonitorOptions) -> Option<String> {
    for relative in ["etc/synoinfo.conf", "etc.defaults/synoinfo.conf"] {
        if let Ok(content) = fs::read_to_string(options.root_path(relative))
            && let Some(name) = parse_synoinfo(&content)
        {
            return Some(name);
        }
    }

    options
        .root_path("usr/syno")
        .is_dir()
        .then(|| "Synology DSM".to_owned())
}

#[cfg(target_os = "linux")]
fn parse_synoinfo(content: &str) -> Option<String> {
    let mut unique = None;
    let mut check_state = None;

    for line in content.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("unique=") {
            unique = Some(value.trim_matches('"').to_owned());
        } else if let Some(value) = line.strip_prefix("udc_check_state=") {
            check_state = Some(value.trim_matches('"').to_owned());
        }
    }

    let unique = unique?;
    if !unique.contains("synology_") {
        return None;
    }
    let parts: Vec<&str> = unique.split('_').collect();
    if parts.len() < 3 {
        return None;
    }

    let model = parts.last()?.to_ascii_uppercase();
    let mut result = format!("Synology {model} DSM");
    if let Some(state) = check_state.filter(|state| !state.is_empty()) {
        result.push(' ');
        result.push_str(&state);
    }
    Some(result)
}

/// fnOS detection mirrors Go: `BUILD_VERSION` first, then the `/usr/trim`
/// directory.
#[cfg(target_os = "linux")]
fn detect_fnos(options: &MonitorOptions) -> Option<String> {
    if let Ok(content) = fs::read_to_string(options.root_path("usr/trim/BUILD_VERSION")) {
        let version = content.trim();
        if !version.is_empty() {
            return Some(format!("fnOS {version}"));
        }
    }

    options
        .root_path("usr/trim")
        .is_dir()
        .then(|| "fnOS".to_owned())
}

#[cfg(target_os = "linux")]
fn is_default_root(options: &MonitorOptions) -> bool {
    options.root == Path::new("/")
}

#[cfg(target_os = "linux")]
fn read_os_release(options: &MonitorOptions) -> Option<String> {
    OS_RELEASE_PATHS
        .iter()
        .filter_map(|relative| fs::read_to_string(options.root_path(relative)).ok())
        .find_map(|content| parse_os_release(&content))
}

#[cfg(target_os = "linux")]
fn read_os_release_value(options: &MonitorOptions, key: &str) -> Option<String> {
    OS_RELEASE_PATHS
        .iter()
        .filter_map(|relative| fs::read_to_string(options.root_path(relative)).ok())
        .find_map(|content| parse_os_release_value(&content, key))
}

/// Extract a value from an `os-release` file.
///
/// Values may be quoted and contain shell-style escapes; the quoting is removed
/// before the value is returned.
fn parse_os_release_value(content: &str, wanted: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == wanted {
            let value = unquote_os_release(value);
            if !value.is_empty() {
                return Some(value);
            }
        }
    }

    None
}

/// Extract `PRETTY_NAME` from an `os-release` file.
pub(crate) fn parse_os_release(content: &str) -> Option<String> {
    parse_os_release_value(content, "PRETTY_NAME")
}

fn unquote_os_release(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let quoted = (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'');
        if quoted {
            return unescape(&value[1..value.len() - 1]);
        }
    }

    value.to_owned()
}

fn unescape(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            result.push(ch);
            continue;
        }
        match chars.next() {
            Some(escaped @ ('"' | '\'' | '\\' | '$' | '`')) => result.push(escaped),
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::{map_arch, parse_os_release};

    #[cfg(target_os = "linux")]
    use super::{
        parse_android_build_prop, parse_proxmox_version, parse_synoinfo, read_os_release_value,
    };

    #[test]
    fn parses_quoted_and_unquoted_pretty_name() {
        let quoted = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nVERSION_ID=\"24.04\"\n";
        assert_eq!(
            parse_os_release(quoted).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );

        let unquoted = "PRETTY_NAME=Debian GNU/Linux 12 (bookworm)\n";
        assert_eq!(
            parse_os_release(unquoted).as_deref(),
            Some("Debian GNU/Linux 12 (bookworm)")
        );

        let escaped = r#"PRETTY_NAME="Example \"Edge\" OS"
"#;
        assert_eq!(
            parse_os_release(escaped).as_deref(),
            Some("Example \"Edge\" OS")
        );
    }

    #[test]
    fn os_release_without_pretty_name_returns_none() {
        assert_eq!(parse_os_release("NAME=Alpine\nVERSION_ID=3.20\n"), None);
        assert_eq!(parse_os_release("# PRETTY_NAME=Commented\n"), None);
        assert_eq!(parse_os_release("PRETTY_NAME=\"\"\n"), None);
        assert_eq!(parse_os_release(""), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_arbitrary_os_release_values() {
        let content = "NAME=Debian\nVERSION_CODENAME=bookworm\n";
        assert_eq!(
            read_os_release_value(
                &crate::options::MonitorOptions::default().with_root("/nonexistent"),
                "VERSION_CODENAME"
            ),
            None
        );
        assert_eq!(
            super::parse_os_release_value(content, "VERSION_CODENAME").as_deref(),
            Some("bookworm")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn special_distros_use_injected_filesystem_root() {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp root");
        let options = crate::options::MonitorOptions::default().with_root(dir.path());

        fs::create_dir_all(dir.path().join("usr/trim")).unwrap();
        fs::write(dir.path().join("usr/trim/BUILD_VERSION"), "1.1.11\n").unwrap();
        assert_eq!(super::os_name(&options), "fnOS 1.1.11");

        fs::remove_dir_all(dir.path().join("usr/trim")).unwrap();
        fs::create_dir_all(dir.path().join("etc")).unwrap();
        fs::write(
            dir.path().join("etc/synoinfo.conf"),
            "unique=\"synology_ds920_plus\"\nudc_check_state=\"1\"\n",
        )
        .unwrap();
        assert_eq!(super::os_name(&options), "Synology PLUS DSM 1");

        fs::remove_file(dir.path().join("etc/synoinfo.conf")).unwrap();
        fs::write(
            dir.path().join("etc/os-release"),
            "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nVERSION_CODENAME=bookworm\n",
        )
        .unwrap();
        assert_eq!(super::os_name(&options), "Debian GNU/Linux 12 (bookworm)");

        let android = TempDir::new().expect("android root");
        fs::create_dir_all(android.path().join("system")).unwrap();
        fs::write(
            android.path().join("system/build.prop"),
            "ro.build.version.release=14\nro.product.model=Pixel 8\nro.product.brand=Google\n",
        )
        .unwrap();
        let options = crate::options::MonitorOptions::default().with_root(android.path());
        assert_eq!(super::os_name(&options), "Android 14 (Google Pixel 8)");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proxmox_binary_is_resolved_below_injected_root() {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp root");
        fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        let binary = dir.path().join("usr/bin/pveversion");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf 'pve-manager/8.2.4/6f5f9f2c7b9c0e1a\\n'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&binary).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&binary, permissions).unwrap();
        }

        fs::create_dir_all(dir.path().join("etc")).unwrap();
        fs::write(
            dir.path().join("etc/os-release"),
            "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nVERSION_CODENAME=bookworm\n",
        )
        .unwrap();

        let options = crate::options::MonitorOptions::default().with_root(dir.path());
        assert_eq!(super::os_name(&options), "Proxmox VE 8.2.4 (bookworm)");
    }

    #[test]
    fn maps_rust_arch_to_goarch() {
        assert_eq!(map_arch("x86_64"), "amd64");
        assert_eq!(map_arch("aarch64"), "arm64");
        assert_eq!(map_arch("x86"), "386");
        assert_eq!(map_arch("loongarch64"), "loong64");
        assert_eq!(map_arch("riscv64"), "riscv64");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_proxmox_version_output() {
        let output = "pve-manager/8.2.4/6f5f9f2c7b9c0e1a\n";
        assert_eq!(parse_proxmox_version(output).as_deref(), Some("8.2.4"));
        assert_eq!(parse_proxmox_version("proxmox-ve: 8.2\n"), None);
        assert_eq!(
            parse_proxmox_version("pve-manager/8.2.4~rc1/abc\n").as_deref(),
            Some("8.2.4")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_synology_info() {
        let content = "unique=\"synology_ds920_plus\"\nudc_check_state=\"1\"\n";
        assert_eq!(
            parse_synoinfo(content).as_deref(),
            Some("Synology PLUS DSM 1")
        );
        assert_eq!(parse_synoinfo("unique=\"other\""), None);
        assert_eq!(parse_synoinfo("unique=\"synology_x\""), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_android_build_prop() {
        let content = "\
ro.build.version.release=14
ro.product.model=Pixel 8
ro.product.brand=Google
";
        assert_eq!(
            parse_android_build_prop(content).as_deref(),
            Some("Android 14 (Google Pixel 8)")
        );
        assert_eq!(parse_android_build_prop("ro.product.model=Pixel\n"), None);
    }
}
