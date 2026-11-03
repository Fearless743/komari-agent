//! Operating system information.

use std::fs;

use crate::options::MonitorOptions;
use crate::platform;
use crate::types::OsInfo;

/// Distribution names read by the Linux implementation, in lookup order.
const OS_RELEASE_PATHS: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];

/// Collect distribution name, kernel version and architecture.
pub(crate) fn collect(options: &MonitorOptions) -> OsInfo {
    OsInfo {
        name: os_name(),
        kernel_version: kernel_version(options),
        arch: arch(),
    }
}

/// Distribution name.
///
/// Linux uses `PRETTY_NAME` from `os-release` and falls back to `Linux`, like
/// Go's `OSName()`. Other platforms use `sysinfo`.
pub(crate) fn os_name() -> String {
    #[cfg(target_os = "linux")]
    {
        read_os_release().unwrap_or_else(platform::fallback::os_name)
    }

    #[cfg(not(target_os = "linux"))]
    {
        platform::fallback::os_name()
    }
}

/// Kernel release.
///
/// On Linux this is read from the injectable proc root
/// (`sys/kernel/osrelease`), so a mounted host proc reports the host kernel.
/// `sysinfo` is used when the file is unavailable.
pub(crate) fn kernel_version(options: &MonitorOptions) -> String {
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

fn read_os_release() -> Option<String> {
    OS_RELEASE_PATHS
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .find_map(|content| parse_os_release(&content))
}

/// Extract `PRETTY_NAME` from an `os-release` file.
///
/// Values may be quoted and contain shell-style escapes; the quoting is removed
/// before the value is returned.
pub(crate) fn parse_os_release(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "PRETTY_NAME" {
            let value = unquote_os_release(value);
            if !value.is_empty() {
                return Some(value);
            }
        }
    }

    None
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

    #[test]
    fn maps_rust_arch_to_goarch() {
        assert_eq!(map_arch("x86_64"), "amd64");
        assert_eq!(map_arch("aarch64"), "arm64");
        assert_eq!(map_arch("x86"), "386");
        assert_eq!(map_arch("loongarch64"), "loong64");
        assert_eq!(map_arch("riscv64"), "riscv64");
    }
}
