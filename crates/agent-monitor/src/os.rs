//! Operating system information.
//!
//! Distribution detection follows the Go agent's order: Android, Proxmox VE,
//! Synology DSM, fnOS, then `os-release` PRETTY_NAME, then `Linux`.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
use std::fs;

use crate::options::MonitorOptions;
use crate::platform;
use crate::types::OsInfo;

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
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
        match fs::read_to_string(options.proc_path("sys/kernel/osrelease")) {
            Ok(value) => {
                let value = value.trim();
                if !value.is_empty() {
                    return value.to_owned();
                }
            }
            Err(error) => {
                platform::warn_proc_failure(
                    options,
                    "sys/kernel/osrelease",
                    &crate::error::MonitorError::io(
                        options.proc_path("sys/kernel/osrelease"),
                        error,
                    ),
                );
            }
        }

        if options.strict_proc {
            return "Unknown".to_owned();
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
    if is_default_root(options)
        && let Some(version) = getprop("ro.build.version.release")
    {
        let model = getprop("ro.product.model").unwrap_or_default();
        let brand = getprop("ro.product.brand").unwrap_or_default();
        return Some(format_android(&version, &model, &brand));
    }

    let build_prop = options.root_path("system/build.prop");
    if build_prop.exists() {
        // Go's readAndroidBuildProp returns "Android" when the file exists but
        // cannot be read or has no release version.
        let name = fs::read_to_string(&build_prop)
            .ok()
            .and_then(|content| parse_android_build_prop(&content));
        return Some(name.unwrap_or_else(|| "Android".to_owned()));
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
    // Only the default root may consult the host PATH. With an injected root,
    // detection must not depend on host tools unless `process_path` is explicit.
    let host_path = if is_default_root(options) {
        std::env::var_os("PATH")
    } else {
        None
    };
    let binary = pveversion_binary(options, host_path.as_deref())?;

    let output = Command::new(binary).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Resolve `pveversion` for the current root.
///
/// `host_path` is only consulted when `options.root` is `/`. For an injected
/// root the binary must come from `<root>/usr/bin/pveversion` or an explicitly
/// injected [`MonitorOptions::process_path`]; otherwise detection is skipped.
#[cfg(target_os = "linux")]
fn pveversion_binary(
    options: &MonitorOptions,
    host_path: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    if is_default_root(options) {
        let path = options.process_path.as_deref().or(host_path);
        return find_in_path("pveversion", path);
    }

    let root_binary = options.root_path("usr/bin/pveversion");
    if is_executable(&root_binary) {
        return Some(root_binary);
    }

    options
        .process_path
        .as_deref()
        .and_then(|path| find_in_path("pveversion", Some(path)))
}

/// Resolve an executable through an injected `PATH` value.
///
/// Split out so the Proxmox PATH lookup can be tested without mutating the
/// process environment. Like Go's `exec.LookPath`, only files with at least one
/// execute bit are accepted.
#[cfg(target_os = "linux")]
fn find_in_path(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = path.filter(|path| !path.is_empty())?;
    std::env::split_paths(path)
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(target_os = "linux")]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Extract `pve-manager/<version>~...` from `pveversion` output.
///
/// Go overwrites the version for every matching line, so the last match wins.
#[cfg(target_os = "linux")]
fn parse_proxmox_version(output: &str) -> Option<String> {
    let mut found = None;
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pve-manager/") else {
            continue;
        };
        let version = rest.split('/').next().unwrap_or(rest);
        let version = version.split('~').next().unwrap_or(version).trim();
        if !version.is_empty() {
            found = Some(version.to_owned());
        }
    }
    found
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
    // Go's Proxmox codename lookup reads only /etc/os-release.
    fs::read_to_string(options.root_path("etc/os-release"))
        .ok()
        .and_then(|content| parse_os_release_value(&content, key))
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

        // Go returns bare "Android" when build.prop exists but has no version.
        fs::write(
            android.path().join("system/build.prop"),
            "ro.product.model=Pixel 8\n",
        )
        .unwrap();
        assert_eq!(super::os_name(&options), "Android");
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
        // Go overwrites the version for each match: the last line wins.
        assert_eq!(
            parse_proxmox_version("pve-manager/8.1.0/aaa\nother\npve-manager/8.2.4/bbb\n")
                .as_deref(),
            Some("8.2.4")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proxmox_codename_reads_only_etc_os_release() {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp root");
        let options = crate::options::MonitorOptions::default().with_root(dir.path());

        fs::create_dir_all(dir.path().join("usr/lib")).unwrap();
        fs::write(
            dir.path().join("usr/lib/os-release"),
            "VERSION_CODENAME=bookworm\n",
        )
        .unwrap();
        assert_eq!(
            super::read_os_release_value(&options, "VERSION_CODENAME"),
            None
        );

        fs::create_dir_all(dir.path().join("etc")).unwrap();
        fs::write(
            dir.path().join("etc/os-release"),
            "VERSION_CODENAME=trixie\n",
        )
        .unwrap();
        assert_eq!(
            super::read_os_release_value(&options, "VERSION_CODENAME").as_deref(),
            Some("trixie")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_only_executable_pveversion_through_injected_path() {
        use std::ffi::OsStr;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("path dir");
        let binary = dir.path().join("pveversion");
        fs::write(&binary, "#!/bin/sh\n").unwrap();

        let path = dir.path().as_os_str();
        // No execute bit: Go's LookPath must reject it.
        assert_eq!(super::find_in_path("pveversion", Some(path)), None);

        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();
        assert_eq!(
            super::find_in_path("pveversion", Some(path)),
            Some(binary.clone())
        );
        assert_eq!(super::find_in_path("missing", Some(path)), None);
        assert_eq!(super::find_in_path("pveversion", None), None);
        assert_eq!(
            super::find_in_path("pveversion", Some(OsStr::new(""))),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proxmox_detection_uses_injected_path_end_to_end() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use tempfile::TempDir;

        let root = TempDir::new().expect("temp root");
        let bin_dir = TempDir::new().expect("bin dir");
        let binary = bin_dir.path().join("pveversion");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf 'pve-manager/8.2.4/6f5f9f2c7b9c0e1a\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();

        fs::create_dir_all(root.path().join("etc")).unwrap();
        fs::write(
            root.path().join("etc/os-release"),
            "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nVERSION_CODENAME=bookworm\n",
        )
        .unwrap();

        let options = crate::options::MonitorOptions::default()
            .with_root(root.path())
            .with_process_path(bin_dir.path().as_os_str());
        assert_eq!(
            super::detect_proxmox(&options).as_deref(),
            Some("Proxmox VE 8.2.4 (bookworm)")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn injected_root_ignores_host_path_for_pve() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use tempfile::TempDir;

        let root = TempDir::new().expect("temp root");
        fs::create_dir_all(root.path().join("etc")).unwrap();
        fs::write(
            root.path().join("etc/os-release"),
            "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nVERSION_CODENAME=bookworm\n",
        )
        .unwrap();

        let host_bin = TempDir::new().expect("host bin");
        let fake = host_bin.path().join("pveversion");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf 'pve-manager/8.2.4/6f5f9f2c7b9c0e1a\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake, permissions).unwrap();

        let options = crate::options::MonitorOptions::default().with_root(root.path());

        // Even with a host PATH containing pveversion, an injected root must
        // not use it unless process_path is explicitly provided.
        assert_eq!(
            super::pveversion_binary(&options, Some(host_bin.path().as_os_str())),
            None
        );
        assert_eq!(super::detect_proxmox(&options), None);
        assert_eq!(super::os_name(&options), "Debian GNU/Linux 12 (bookworm)");

        let explicit = options
            .clone()
            .with_process_path(host_bin.path().as_os_str());
        assert_eq!(
            super::pveversion_binary(&explicit, None),
            Some(host_bin.path().join("pveversion"))
        );
        assert_eq!(
            super::detect_proxmox(&explicit).as_deref(),
            Some("Proxmox VE 8.2.4 (bookworm)")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_root_uses_host_path_but_process_path_overrides_it() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use tempfile::TempDir;

        fn executable_pveversion(dir: &std::path::Path) -> std::path::PathBuf {
            let binary = dir.join("pveversion");
            fs::write(&binary, "#!/bin/sh\n").unwrap();
            let mut permissions = fs::metadata(&binary).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&binary, permissions).unwrap();
            binary
        }

        let host_bin = TempDir::new().expect("host bin");
        let host_binary = executable_pveversion(host_bin.path());
        let override_bin = TempDir::new().expect("override bin");
        let override_binary = executable_pveversion(override_bin.path());

        let options = crate::options::MonitorOptions::default();
        assert_eq!(
            super::pveversion_binary(&options, Some(host_bin.path().as_os_str())),
            Some(host_binary)
        );

        let options = options.with_process_path(override_bin.path().as_os_str());
        assert_eq!(
            super::pveversion_binary(&options, Some(host_bin.path().as_os_str())),
            Some(override_binary)
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
