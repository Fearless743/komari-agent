//! Virtualization / container detection.
//!
//! Linux follows the Go probe order: `systemd-detect-virt`, container markers
//! and cgroups, DMI, then the CPU `hypervisor` flag. Bare metal returns
//! `none`; when no probe can run, an empty string is returned.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::process::Command;

use crate::options::MonitorOptions;

/// Detect the hypervisor or container runtime.
///
/// Returns an empty string when detection is not possible (for example a
/// stripped-down container without `/proc` and without DMI access).
pub fn detect_virtualization(options: &MonitorOptions) -> String {
    #[cfg(target_os = "linux")]
    {
        detect_linux(options, systemd_detect_virt())
    }

    #[cfg(not(target_os = "linux"))]
    {
        platform_detect(options)
    }
}

/// Linux probe order with the `systemd-detect-virt` result injected.
///
/// Passing `None` exercises the path used when the binary is unavailable,
/// which is also how tests avoid depending on the host.
#[cfg(target_os = "linux")]
fn detect_linux(options: &MonitorOptions, systemd: Option<String>) -> String {
    if let Some(kind) = systemd {
        return kind;
    }
    if let Some(kind) = detect_container(options) {
        return kind;
    }
    if let Some(kind) = detect_dmi(options) {
        return kind;
    }
    if let Some(kind) = detect_cpu_hypervisor(options) {
        return kind;
    }
    if options.proc_path("cpuinfo").is_file() {
        return "none".to_owned();
    }
    String::new()
}

#[cfg(target_os = "linux")]
fn systemd_detect_virt() -> Option<String> {
    let output = Command::new("systemd-detect-virt").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let kind = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if kind.is_empty() { None } else { Some(kind) }
}

#[cfg(target_os = "linux")]
fn detect_container(options: &MonitorOptions) -> Option<String> {
    if options.root_path(".dockerenv").is_file() {
        return Some("docker".to_owned());
    }

    let cgroup = fs::read_to_string(options.proc_path("self/cgroup")).ok();

    if options.root_path("run/.containerenv").is_file() {
        return Some(
            cgroup
                .as_deref()
                .and_then(container_from_cgroup)
                .unwrap_or_else(|| "container".to_owned()),
        );
    }

    if let Some(kind) = cgroup.as_deref().and_then(container_from_cgroup) {
        return Some(kind);
    }

    if options.root_path("dev/.lxc-boot-id").is_file() {
        return Some("lxc".to_owned());
    }
    if options.root_path(".komari-agent-container").is_file() {
        return Some("container".to_owned());
    }

    None
}

/// Classify a `/proc/self/cgroup` payload.
///
/// The checks are intentionally ordered from specific runtime to generic
/// container, mirroring the Go implementation.
pub(crate) fn container_from_cgroup(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();

    let kind = if lower.contains("/docker/")
        || lower.contains("/docker-")
        || lower.contains("cri-containerd")
    {
        "docker"
    } else if lower.contains("libpod") || lower.contains("podman") {
        "podman"
    } else if lower.contains("crio-") {
        "container"
    } else if lower.contains("kubepods") {
        "kubernetes"
    } else if lower.contains("/lxc/") {
        "lxc"
    } else {
        return None;
    };

    Some(kind.to_owned())
}

#[cfg(target_os = "linux")]
fn detect_dmi(options: &MonitorOptions) -> Option<String> {
    let fields: Vec<String> = ["product_name", "sys_vendor", "board_vendor", "bios_vendor"]
        .iter()
        .filter_map(|name| {
            fs::read_to_string(options.sys_path(format!("class/dmi/id/{name}"))).ok()
        })
        .collect();

    let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
    virt_from_dmi_fields(&fields)
}

/// Map DMI strings (product name, vendor, board vendor, BIOS vendor) to a
/// systemd-detect-virt compatible name.
pub(crate) fn virt_from_dmi_fields(fields: &[&str]) -> Option<String> {
    let text = fields
        .iter()
        .map(|field| field.trim())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();

    if text.is_empty() {
        return None;
    }

    // More specific virtualizers before generic ones: KVM guests often expose
    // "QEMU" in DMI, so the KVM marker must win when present.
    const MAPPING: [(&str, &str); 15] = [
        ("kvm", "kvm"),
        ("vmware", "vmware"),
        ("virtualbox", "oracle"),
        ("innotek", "oracle"),
        ("hyper-v", "microsoft"),
        ("microsoft", "microsoft"),
        ("xen", "xen"),
        ("qemu", "qemu"),
        ("bochs", "bochs"),
        ("parallels", "parallels"),
        ("openstack", "openstack"),
        ("amazon", "amazon"),
        ("google", "google"),
        ("digitalocean", "digitalocean"),
        ("alibaba", "alibaba"),
    ];

    for (needle, name) in MAPPING {
        if text.contains(needle) {
            return Some(name.to_owned());
        }
    }

    if text.contains("virtual") {
        return Some("virtualized".to_owned());
    }

    None
}

#[cfg(target_os = "linux")]
fn detect_cpu_hypervisor(options: &MonitorOptions) -> Option<String> {
    let content = fs::read_to_string(options.proc_path("cpuinfo")).ok()?;

    let has_hypervisor = content.lines().any(|line| {
        let Some((key, value)) = line.split_once(':') else {
            return false;
        };
        let key = key.trim();
        (key == "flags" || key == "Features")
            && value
                .split_whitespace()
                .any(|flag| flag.eq_ignore_ascii_case("hypervisor"))
    });

    has_hypervisor.then(|| "virtualized".to_owned())
}

#[cfg(target_os = "macos")]
fn platform_detect(_options: &MonitorOptions) -> String {
    let output = Command::new("sysctl")
        .args(["-n", "kern.hv_vmm_present"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            match String::from_utf8_lossy(&output.stdout).trim() {
                "1" => "virtualized".to_owned(),
                "0" => "none".to_owned(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

#[cfg(target_os = "freebsd")]
fn platform_detect(_options: &MonitorOptions) -> String {
    let output = Command::new("sysctl")
        .args(["-n", "kern.vm_guest"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => String::new(),
    }
}

#[cfg(windows)]
fn platform_detect(_options: &MonitorOptions) -> String {
    // Detecting Hyper-V requires CPUID or WMI. Windows support is a graceful
    // fallback in this task; report an empty value rather than a wrong one.
    String::new()
}

#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "macos"),
    not(target_os = "freebsd"),
    not(windows)
))]
fn platform_detect(_options: &MonitorOptions) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::{container_from_cgroup, virt_from_dmi_fields};

    #[test]
    fn classifies_container_cgroups() {
        assert_eq!(
            container_from_cgroup(
                "12:devices:/docker/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
            )
            .as_deref(),
            Some("docker")
        );
        assert_eq!(
            container_from_cgroup("0::/machine.slice/libpod-abc123.scope\n").as_deref(),
            Some("podman")
        );
        assert_eq!(
            container_from_cgroup("0::/kubepods/burstable/pod1234/abc\n").as_deref(),
            Some("kubernetes")
        );
        assert_eq!(
            container_from_cgroup("0::/crio-0123456789abcdef.scope\n").as_deref(),
            Some("container")
        );
        assert_eq!(
            container_from_cgroup("0::/lxc/example\n").as_deref(),
            Some("lxc")
        );
        assert_eq!(
            container_from_cgroup("0::/user.slice/user-1000.slice/session-1.scope\n"),
            None
        );
        assert_eq!(container_from_cgroup(""), None);
    }

    #[test]
    fn maps_dmi_vendors_to_systemd_names() {
        assert_eq!(
            virt_from_dmi_fields(&["KVM", "QEMU", "Red Hat", "SeaBIOS"]).as_deref(),
            Some("kvm")
        );
        assert_eq!(
            virt_from_dmi_fields(&["VirtualBox", "innotek GmbH", "", ""]).as_deref(),
            Some("oracle")
        );
        assert_eq!(
            virt_from_dmi_fields(&["Virtual Machine", "Microsoft Corporation", "", ""]).as_deref(),
            Some("microsoft")
        );
        assert_eq!(
            virt_from_dmi_fields(&["VMware Virtual Platform", "VMware, Inc.", "", ""]).as_deref(),
            Some("vmware")
        );
        assert_eq!(
            virt_from_dmi_fields(&["Standard PC", "QEMU", "", ""]).as_deref(),
            Some("qemu")
        );
        assert_eq!(
            virt_from_dmi_fields(&["Some Virtual Appliance", "", "", ""]).as_deref(),
            Some("virtualized")
        );
        assert_eq!(virt_from_dmi_fields(&["", "  ", "", ""]), None);
        assert_eq!(
            virt_from_dmi_fields(&["ThinkPad X1", "LENOVO", "", ""]),
            None
        );
    }

    #[cfg(target_os = "linux")]
    fn injected_options(root: &std::path::Path) -> crate::options::MonitorOptions {
        crate::options::MonitorOptions::default()
            .with_root(root)
            .with_proc_root(root.join("proc"))
            .with_sys_root(root.join("sys"))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_systemd_uses_injected_container_and_cpu_probes() {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp root");
        let options = injected_options(dir.path());

        // No systemd, no markers, no DMI, no cpuinfo -> empty, never panic.
        assert_eq!(super::detect_linux(&options, None), "");

        fs::create_dir_all(dir.path().join("proc/self")).unwrap();
        fs::write(
            dir.path().join("proc/self/cgroup"),
            "12:devices:/docker/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .unwrap();
        assert_eq!(super::detect_linux(&options, None), "docker");

        fs::remove_file(dir.path().join("proc/self/cgroup")).unwrap();
        fs::write(
            dir.path().join("proc/cpuinfo"),
            "processor\t: 0\nflags\t\t: fpu vme hypervisor\n",
        )
        .unwrap();
        assert_eq!(super::detect_linux(&options, None), "virtualized");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_systemd_uses_injected_dmi_and_explicit_result() {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp root");
        let options = injected_options(dir.path());

        let dmi = dir.path().join("sys/class/dmi/id");
        fs::create_dir_all(&dmi).unwrap();
        fs::write(dmi.join("product_name"), "KVM\n").unwrap();
        fs::write(dmi.join("sys_vendor"), "QEMU\n").unwrap();
        assert_eq!(super::detect_linux(&options, None), "kvm");

        // An explicit systemd result is authoritative.
        assert_eq!(
            super::detect_linux(&options, Some("vmware".to_owned())),
            "vmware"
        );
    }
}
