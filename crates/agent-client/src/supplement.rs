//! Supplemental metrics not yet owned by `agent-monitor`.
//!
//! A03 is scheduled to add disk, network, GPU, connection and process
//! collection to `agent-monitor`. Until that crate lands, the reporting loop
//! needs those values to produce a Go-compatible v1 payload, so this module
//! implements the Linux subset required by `GenerateReport` and the basic info
//! upload. The code is intentionally isolated behind [`SystemSupplement`] so it
//! can be deleted once A03 provides the same data through
//! `agent_monitor::Collector`.
//!
//! Only Linux reads `/proc` and `/sys`; other platforms report zero/absent
//! values (the same visible effect as a failed probe in Go).
//!
//! # Network sampling
//!
//! Go's `NetworkSpeed` sleeps for one second between two counter reads. The
//! reporting loop instead reads the first sample before the (also one second)
//! CPU window and the second sample afterwards, so the report still costs one
//! second and the rates cover the real elapsed interval.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_protocol::{GpuDeviceInfoV1, GpuReportV1};
use globset::Glob;

use crate::report::{ConnectionSample, DiskUsage, NetworkSample, gpu_models_report};

/// Options for supplemental collection.
#[derive(Clone, Debug)]
pub struct SupplementOptions {
    pub proc_root: PathBuf,
    pub sys_root: PathBuf,
    pub include_nics: String,
    pub exclude_nics: String,
    pub include_mountpoints: String,
    pub enable_gpu: bool,
    pub custom_ipv4: String,
    pub custom_ipv6: String,
    pub get_ip_addr_from_nic: bool,
}

impl Default for SupplementOptions {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            sys_root: PathBuf::from("/sys"),
            include_nics: String::new(),
            exclude_nics: String::new(),
            include_mountpoints: String::new(),
            enable_gpu: false,
            custom_ipv4: String::new(),
            custom_ipv6: String::new(),
            get_ip_addr_from_nic: false,
        }
    }
}

/// One network counter sample.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkTotals {
    pub up: u64,
    pub down: u64,
}

/// Result of supplemental collection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupplementSnapshot {
    pub disk: DiskUsage,
    pub network: NetworkSample,
    pub connections: ConnectionSample,
    pub process: i64,
    pub gpu: Option<GpuReportV1>,
    pub gpu_name: String,
    pub ipv4: String,
    pub ipv6: String,
    /// Errors are returned separately so the provider can append them in Go's
    /// order: network, connections, uptime, GPU.
    pub network_error: Option<String>,
    pub connections_error: Option<String>,
    pub gpu_error: Option<String>,
}

/// Fields used by the basic information upload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BasicSupplement {
    pub disk_total: u64,
    pub gpu_name: String,
    pub ipv4: String,
    pub ipv6: String,
}

/// A partition as printed by `list-disk`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionInfo {
    pub device: String,
    pub mountpoint: String,
    pub fstype: String,
}

/// Supplemental collector.
#[derive(Clone, Debug)]
pub struct SystemSupplement {
    options: SupplementOptions,
}

impl SystemSupplement {
    pub fn new(options: SupplementOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &SupplementOptions {
        &self.options
    }

    /// First network counter sample, used before the CPU sampling window.
    pub fn network_totals(&self) -> Result<NetworkTotals, String> {
        network::totals(&self.options)
    }

    /// Collect disk/network/GPU/connection/process data.
    ///
    /// `previous` is the sample taken before the CPU window; `elapsed` is the
    /// time between that sample and now. When `previous` is `None`, the network
    /// block is still reported with zero rates.
    pub fn collect(
        &self,
        previous: Option<NetworkTotals>,
        elapsed: Duration,
    ) -> SupplementSnapshot {
        // Take the second counter sample immediately after the CPU window,
        // before any potentially slow disk probing, so the rate divisor and
        // the counters describe the same interval.
        let network_result = self.network_totals();

        // Go ignores disk errors (it reports zero usage silently).
        let disk = disk::usage(&self.options).unwrap_or_default();

        let mut network_error = None;
        let mut connections_error = None;
        let mut gpu_error = None;

        let network = match (previous, network_result) {
            (Some(start), Ok(end)) => network_sample(start, end, elapsed),
            (None, Ok(end)) => NetworkSample {
                total_up: end.up,
                total_down: end.down,
                ..NetworkSample::default()
            },
            (_, Err(error)) => {
                network_error = Some(error);
                NetworkSample::default()
            }
        };

        let connections = match connections::count(&self.options) {
            Ok(count) => count,
            Err(error) => {
                connections_error = Some(error);
                ConnectionSample::default()
            }
        };

        let process = process::count(&self.options);

        let models = gpu::models(&self.options);
        let gpu_name = models.first().cloned().unwrap_or_else(|| "None".to_owned());

        let gpu = if self.options.enable_gpu {
            match gpu::detailed(&self.options) {
                Ok(devices) if !devices.is_empty() => {
                    Some(crate::report::detailed_gpu_report(&devices))
                }
                Ok(_) => gpu_models_report(models.clone()),
                Err(error) => {
                    gpu_error = Some(error);
                    gpu_models_report(models.clone())
                }
            }
        } else {
            None
        };

        let (ipv4, ipv6) = ip::addresses(&self.options);

        SupplementSnapshot {
            disk,
            network,
            connections,
            process,
            gpu,
            gpu_name,
            ipv4,
            ipv6,
            network_error,
            connections_error,
            gpu_error,
        }
    }

    /// Basic information fields that do not require the network window.
    pub fn basic(&self) -> BasicSupplement {
        let disk_total = disk::usage(&self.options)
            .map(|usage| usage.total)
            .unwrap_or(0);
        let models = gpu::models(&self.options);
        let (ipv4, ipv6) = ip::addresses(&self.options);

        BasicSupplement {
            disk_total,
            gpu_name: models.first().cloned().unwrap_or_else(|| "None".to_owned()),
            ipv4,
            ipv6,
        }
    }
}

/// List all partitions for the `list-disk` command.
pub fn list_partitions(proc_root: &Path) -> Result<Vec<PartitionInfo>, String> {
    disk::all_partitions(proc_root)
}

/// Compute the network rate and totals from two counter samples.
fn network_sample(start: NetworkTotals, end: NetworkTotals, elapsed: Duration) -> NetworkSample {
    let seconds = elapsed.as_secs_f64().max(0.001);
    NetworkSample {
        up: ((end.up.saturating_sub(start.up)) as f64 / seconds) as u64,
        down: ((end.down.saturating_sub(start.down)) as f64 / seconds) as u64,
        total_up: end.up,
        total_down: end.down,
    }
}

/// Port of Go's `monitoring.DiskList()`: physical partitions, one per device,
/// keeping the shortest mountpoint, formatted as `mountpoint (fstype)`.
pub fn monitoring_mountpoints(
    proc_root: &Path,
    include_mountpoints: &str,
) -> Result<Vec<String>, String> {
    let include = include_mountpoints.trim();
    if !include.is_empty() {
        return Ok(include
            .split(';')
            .map(str::trim)
            .filter(|mountpoint| !mountpoint.is_empty())
            .map(str::to_owned)
            .collect());
    }

    let mut devices: Vec<(String, PartitionInfo)> = Vec::new();
    for partition in disk::all_partitions(proc_root)? {
        if !disk::is_physical(&partition) {
            continue;
        }

        let key = if partition.fstype.eq_ignore_ascii_case("zfs") {
            partition
                .device
                .split('/')
                .next()
                .unwrap_or(&partition.device)
                .to_owned()
        } else {
            partition.device.clone()
        };

        match devices.iter_mut().find(|(device, _)| *device == key) {
            Some((_, existing)) => {
                if partition.mountpoint.len() < existing.mountpoint.len() {
                    *existing = partition;
                }
            }
            None => devices.push((key, partition)),
        }
    }

    Ok(devices
        .into_iter()
        .map(|(_, partition)| format!("{} ({})", partition.mountpoint, partition.fstype))
        .collect())
}

fn glob_matches(patterns: &str, value: &str) -> bool {
    patterns
        .split(',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| Glob::new(pattern).is_ok_and(|glob| glob.compile_matcher().is_match(value)))
}

fn nic_selected(options: &SupplementOptions, name: &str) -> bool {
    // Go always excludes loopback and virtual/container bridge prefixes.
    const LOOPBACK: &[&str] = &[
        "lo", "veth", "docker", "br", "cni", "podman", "flannel", "virbr", "vmbr", "tap", "fwbr",
        "fwpr",
    ];
    if LOOPBACK.iter().any(|prefix| name.starts_with(prefix)) {
        return false;
    }

    let include = options.include_nics.trim();
    if !include.is_empty() {
        return glob_matches(include, name);
    }

    !glob_matches(&options.exclude_nics, name)
}

#[cfg(target_os = "linux")]
fn read_to_string(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
}

mod network {
    use super::*;

    pub(super) fn totals(options: &SupplementOptions) -> Result<NetworkTotals, String> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            return Ok(NetworkTotals::default());
        }

        #[cfg(target_os = "linux")]
        {
            let path = options.proc_root.join("net/dev");
            let content = read_to_string(&path)?;
            let mut totals = NetworkTotals::default();

            for line in content.lines().skip(2) {
                let Some((name, rest)) = line.split_once(':') else {
                    continue;
                };
                let name = name.trim();
                if !nic_selected(options, name) {
                    continue;
                }

                let fields: Vec<&str> = rest.split_whitespace().collect();
                if fields.len() < 9 {
                    continue;
                }
                let rx = fields[0].parse::<u64>().unwrap_or(0);
                let tx = fields[8].parse::<u64>().unwrap_or(0);
                totals.down = totals.down.saturating_add(rx);
                totals.up = totals.up.saturating_add(tx);
            }

            Ok(totals)
        }
    }
}

mod connections {
    use super::*;

    pub(super) fn count(options: &SupplementOptions) -> Result<ConnectionSample, String> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            return Ok(ConnectionSample::default());
        }

        #[cfg(target_os = "linux")]
        {
            let tcp = count_files(&options.proc_root, &["tcp", "tcp6"])?;
            let udp = count_files(&options.proc_root, &["udp", "udp6"])?;
            Ok(ConnectionSample { tcp, udp })
        }
    }

    #[cfg(target_os = "linux")]
    fn count_files(root: &Path, names: &[&str]) -> Result<i64, String> {
        let mut total = 0i64;
        let mut read_any = false;

        for name in names {
            let path = root.join("net").join(name);
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(format!("{}: {err}", path.display())),
            };
            read_any = true;
            total += content
                .lines()
                .skip(1)
                .filter(|line| !line.trim().is_empty())
                .count() as i64;
        }

        if read_any {
            Ok(total)
        } else {
            Err(format!(
                "no proc net files found under {}",
                root.join("net").display()
            ))
        }
    }
}

mod process {
    use super::*;

    pub(super) fn count(options: &SupplementOptions) -> i64 {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            0
        }

        #[cfg(target_os = "linux")]
        {
            std::fs::read_dir(&options.proc_root)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_str()
                                .is_some_and(|name| name.parse::<u32>().is_ok())
                        })
                        .count() as i64
                })
                .unwrap_or(0)
        }
    }
}

mod disk {
    use super::*;

    #[cfg(target_os = "linux")]
    pub(super) fn all_partitions(proc_root: &Path) -> Result<Vec<PartitionInfo>, String> {
        let content = read_to_string(&proc_root.join("mounts"))?;
        Ok(content.lines().filter_map(parse_mount_line).collect())
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn all_partitions(_proc_root: &Path) -> Result<Vec<PartitionInfo>, String> {
        Ok(Vec::new())
    }

    pub(super) fn usage(options: &SupplementOptions) -> Result<DiskUsage, String> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            return Ok(DiskUsage::default());
        }

        #[cfg(target_os = "linux")]
        {
            let include = options.include_mountpoints.trim();
            if !include.is_empty() {
                let mut total = 0u64;
                let mut used = 0u64;
                for mountpoint in include.split(';').map(str::trim).filter(|m| !m.is_empty()) {
                    if let Some((mount_total, mount_used)) = statvfs(mountpoint) {
                        total = total.saturating_add(mount_total);
                        used = used.saturating_add(mount_used);
                    }
                }
                return Ok(DiskUsage { total, used });
            }

            let partitions = all_partitions(&options.proc_root)?;

            // Deduplicate by device, keeping the largest usage. Go does this to
            // avoid counting bind mounts and ZFS datasets twice.
            let mut devices: Vec<(String, u64, u64)> = Vec::new();
            for partition in partitions {
                if !is_physical(&partition) {
                    continue;
                }
                let Some((total, used)) = statvfs(&partition.mountpoint) else {
                    continue;
                };
                let key = if partition.fstype.eq_ignore_ascii_case("zfs") {
                    partition
                        .device
                        .split('/')
                        .next()
                        .unwrap_or(&partition.device)
                        .to_owned()
                } else {
                    partition.device.clone()
                };

                match devices.iter_mut().find(|(device, _, _)| *device == key) {
                    Some((_, existing_total, existing_used)) => {
                        if total > *existing_total {
                            *existing_total = total;
                            *existing_used = used;
                        }
                    }
                    None => devices.push((key, total, used)),
                }
            }

            let mut result = DiskUsage::default();
            for (_, total, used) in devices {
                result.total = result.total.saturating_add(total);
                result.used = result.used.saturating_add(used);
            }
            Ok(result)
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn parse_mount_line(line: &str) -> Option<PartitionInfo> {
        let mut fields = line.split_whitespace();
        let device = unescape_mount(fields.next()?);
        let mountpoint = unescape_mount(fields.next()?);
        let fstype = fields.next()?.to_owned();
        Some(PartitionInfo {
            device,
            mountpoint,
            fstype,
        })
    }

    #[cfg(target_os = "linux")]
    fn unescape_mount(value: &str) -> String {
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\")
    }

    /// Port of Go's `isPhysicalDisk`.
    pub(super) fn is_physical(partition: &PartitionInfo) -> bool {
        if partition.mountpoint == "/" {
            return true;
        }

        const EXCLUDED_MOUNTS: &[&str] = &[
            "/tmp",
            "/var/tmp",
            "/dev",
            "/run",
            "/var/lib/containers",
            "/var/lib/docker",
            "/proc",
            "/sys",
            "/sys/fs/cgroup",
            "/etc/resolv.conf",
            "/etc/host",
            "/nix/store",
        ];
        let mountpoint = partition.mountpoint.to_lowercase();
        if EXCLUDED_MOUNTS
            .iter()
            .any(|prefix| mountpoint == *prefix || mountpoint.starts_with(prefix))
        {
            return false;
        }

        let fstype = partition.fstype.to_lowercase();
        if fstype == "autofs" && !partition.device.starts_with("/dev/") {
            return false;
        }
        if fstype == "fuseblk" {
            return true;
        }

        const EXCLUDED_FS: &[&str] = &[
            "tmpfs",
            "devtmpfs",
            "udev",
            "nfs",
            "cifs",
            "smb",
            "vboxsf",
            "9p",
            "fuse",
            "overlay",
            "proc",
            "devpts",
            "sysfs",
            "cgroup",
            "mqueue",
            "hugetlbfs",
            "debugfs",
            "binfmt_misc",
            "securityfs",
        ];
        if EXCLUDED_FS
            .iter()
            .any(|excluded| fstype == *excluded || fstype.starts_with(excluded))
        {
            return false;
        }

        !partition.device.starts_with("/dev/loop")
    }

    #[cfg(target_os = "linux")]
    fn statvfs(path: &str) -> Option<(u64, u64)> {
        use std::ffi::CString;

        let path = CString::new(path).ok()?;
        // SAFETY: `statvfs` only writes into the provided struct.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }

        let total = (stat.f_blocks as u64).saturating_mul(stat.f_frsize as u64);
        let free = (stat.f_bfree as u64).saturating_mul(stat.f_frsize as u64);
        Some((total, total.saturating_sub(free)))
    }

    #[cfg(not(target_os = "linux"))]
    fn statvfs(_path: &str) -> Option<(u64, u64)> {
        None
    }
}

mod gpu {
    use super::*;

    pub(super) fn models(options: &SupplementOptions) -> Vec<String> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            Vec::new()
        }

        #[cfg(target_os = "linux")]
        let _ = options;

        #[cfg(target_os = "linux")]
        {
            let output = std::process::Command::new("lspci").output();
            let Ok(output) = output else {
                return Vec::new();
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let mut models = Vec::new();

            for line in text.lines() {
                let lower = line.to_lowercase();
                if !lower.contains("vga") && !lower.contains("3d") && !lower.contains("display") {
                    continue;
                }
                if is_virtual_gpu(&lower) {
                    continue;
                }
                let Some(name) = extract_model(line) else {
                    continue;
                };
                if !models.contains(&name) {
                    models.push(name);
                }
            }

            models
        }
    }

    #[cfg(target_os = "linux")]
    fn is_virtual_gpu(lower: &str) -> bool {
        lower.starts_with("1111")
            || lower.contains("cirrus logic")
            || lower.contains("virtio")
            || lower.contains("vmware")
            || lower.contains("qxl")
            || lower.contains("hyper-v")
    }

    #[cfg(target_os = "linux")]
    fn extract_model(line: &str) -> Option<String> {
        let (_, name) = line.rsplit_once(':')?;
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let name = name
            .rsplit_once('(')
            .map(|(name, _)| name.trim())
            .unwrap_or(name);
        Some(name.to_owned())
    }

    pub(super) fn detailed(options: &SupplementOptions) -> Result<Vec<GpuDeviceInfoV1>, String> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            Err("detailed GPU monitoring is not supported on this platform".to_owned())
        }

        #[cfg(target_os = "linux")]
        let _ = options;

        #[cfg(target_os = "linux")]
        {
            let output = std::process::Command::new("nvidia-smi")
                .args([
                    "--query-gpu=name,memory.total,memory.used,utilization.gpu,temperature.gpu",
                    "--format=csv,noheader,nounits",
                ])
                .output()
                .map_err(|err| format!("nvidia-smi: {err}"))?;

            if !output.status.success() {
                return Err(format!(
                    "nvidia-smi exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }

            let text = String::from_utf8_lossy(&output.stdout);
            let mut devices = Vec::new();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let fields: Vec<&str> = line.split(',').map(str::trim).collect();
                if fields.len() < 5 {
                    continue;
                }
                devices.push(GpuDeviceInfoV1 {
                    name: fields[0].to_owned(),
                    memory_total: fields[1]
                        .parse::<u64>()
                        .unwrap_or(0)
                        .saturating_mul(1024 * 1024),
                    memory_used: fields[2]
                        .parse::<u64>()
                        .unwrap_or(0)
                        .saturating_mul(1024 * 1024),
                    utilization: fields[3].parse::<f64>().unwrap_or(0.0),
                    temperature: fields[4].parse::<u64>().unwrap_or(0),
                    ..GpuDeviceInfoV1::default()
                });
            }

            Ok(devices)
        }
    }
}

mod ip {
    use super::*;

    pub(super) fn addresses(options: &SupplementOptions) -> (String, String) {
        let (nic_ipv4, nic_ipv6) = if options.get_ip_addr_from_nic {
            nic_addresses(options)
        } else {
            (String::new(), String::new())
        };

        select_addresses(
            &options.custom_ipv4,
            &options.custom_ipv6,
            nic_ipv4,
            nic_ipv6,
        )
    }

    /// Go's priority: NIC addresses (when enabled and found) win over custom
    /// values; the external lookup service is intentionally not called.
    pub(super) fn select_addresses(
        custom_ipv4: &str,
        custom_ipv6: &str,
        nic_ipv4: String,
        nic_ipv6: String,
    ) -> (String, String) {
        if !nic_ipv4.is_empty() || !nic_ipv6.is_empty() {
            return (nic_ipv4, nic_ipv6);
        }
        (custom_ipv4.to_owned(), custom_ipv6.to_owned())
    }

    #[cfg(unix)]
    fn nic_addresses(options: &SupplementOptions) -> (String, String) {
        use std::ffi::CStr;
        use std::net::{Ipv4Addr, Ipv6Addr};

        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs either fails without touching `head` or returns a
        // linked list released exactly once with freeifaddrs.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return (String::new(), String::new());
        }

        let mut ipv4 = String::new();
        let mut ipv6 = String::new();
        let mut current = head;

        while !current.is_null() {
            // SAFETY: `current` is a valid node from the getifaddrs list.
            let entry = unsafe { &*current };

            if !entry.ifa_addr.is_null() {
                // SAFETY: ifa_name is a NUL-terminated string owned by the
                // ifaddrs entry for the duration of the list.
                let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy();
                // SAFETY: ifa_addr points to a sockaddr whose family decides
                // which concrete layout is valid.
                let family = unsafe { (*entry.ifa_addr).sa_family as i32 };

                if nic_selected(options, &name)
                    && interface_flags_allow_ip(entry.ifa_flags as libc::c_uint)
                {
                    if family == libc::AF_INET {
                        // SAFETY: AF_INET guarantees sockaddr_in layout.
                        let sockaddr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                        let address = Ipv4Addr::from(u32::from_be(sockaddr.sin_addr.s_addr));
                        if ipv4.is_empty() && !address.is_loopback() {
                            ipv4 = address.to_string();
                        }
                    } else if family == libc::AF_INET6 {
                        // SAFETY: AF_INET6 guarantees sockaddr_in6 layout.
                        let sockaddr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                        let address = Ipv6Addr::from(sockaddr.sin6_addr.s6_addr);
                        if ipv6.is_empty()
                            && !address.is_loopback()
                            && !address.is_unicast_link_local()
                        {
                            ipv6 = address.to_string();
                        }
                    }
                }
            }

            current = entry.ifa_next;
        }

        // SAFETY: head was returned by getifaddrs and is released once.
        unsafe { libc::freeifaddrs(head) };
        (ipv4, ipv6)
    }

    #[cfg(not(unix))]
    fn nic_addresses(options: &SupplementOptions) -> (String, String) {
        let mut ipv4 = String::new();
        let mut ipv6 = String::new();

        if let Ok(interfaces) = if_addrs::get_if_addrs() {
            for interface in interfaces {
                if !nic_selected(options, &interface.name) || !interface.is_oper_up() {
                    continue;
                }
                match interface.ip() {
                    std::net::IpAddr::V4(address) => {
                        if ipv4.is_empty() && !address.is_loopback() {
                            ipv4 = address.to_string();
                        }
                    }
                    std::net::IpAddr::V6(address) => {
                        if ipv6.is_empty()
                            && !address.is_loopback()
                            && !address.is_unicast_link_local()
                        {
                            ipv6 = address.to_string();
                        }
                    }
                }
            }
        }

        (ipv4, ipv6)
    }

    /// Go uses `net.FlagUp` (IFF_UP), not IFF_RUNNING. An administratively up
    /// interface without carrier is still eligible.
    #[cfg(unix)]
    pub(super) fn interface_flags_allow_ip(flags: libc::c_uint) -> bool {
        let up = libc::IFF_UP as libc::c_uint;
        let loopback = libc::IFF_LOOPBACK as libc::c_uint;
        (flags & up) != 0 && (flags & loopback) == 0
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_container_interfaces_are_excluded() {
        let options = SupplementOptions::default();
        for name in ["lo", "veth123", "docker0", "br-abc", "cni0", "tap0"] {
            assert!(!nic_selected(&options, name), "{name}");
        }
        assert!(nic_selected(&options, "eth0"));
        assert!(nic_selected(&options, "enp1s0"));
    }

    #[test]
    fn include_and_exclude_patterns_match_go_wildcards() {
        let options = SupplementOptions {
            include_nics: "eth*".to_owned(),
            ..SupplementOptions::default()
        };
        assert!(nic_selected(&options, "eth0"));
        assert!(!nic_selected(&options, "enp1s0"));

        let options = SupplementOptions {
            exclude_nics: "enp*".to_owned(),
            ..SupplementOptions::default()
        };
        assert!(!nic_selected(&options, "enp1s0"));
        assert!(nic_selected(&options, "eth0"));
    }

    #[test]
    fn virtual_filesystems_are_not_counted() {
        let partition = |mountpoint: &str, fstype: &str, device: &str| PartitionInfo {
            device: device.to_owned(),
            mountpoint: mountpoint.to_owned(),
            fstype: fstype.to_owned(),
        };

        assert!(disk::is_physical(&partition("/", "ext4", "/dev/sda1")));
        assert!(!disk::is_physical(&partition("/proc", "proc", "proc")));
        assert!(!disk::is_physical(&partition(
            "/var/lib/docker",
            "ext4",
            "/dev/sda1"
        )));
        assert!(!disk::is_physical(&partition("/mnt", "tmpfs", "tmpfs")));
        assert!(!disk::is_physical(&partition("/mnt", "ext4", "/dev/loop0")));
        assert!(disk::is_physical(&partition(
            "/mnt",
            "fuseblk",
            "/dev/sdb1"
        )));
    }

    #[test]
    fn nic_addresses_win_over_custom_values_like_go() {
        assert_eq!(
            ip::select_addresses("custom4", "custom6", "nic4".to_owned(), String::new()),
            ("nic4".to_owned(), String::new())
        );
        assert_eq!(
            ip::select_addresses("custom4", "custom6", String::new(), String::new()),
            ("custom4".to_owned(), "custom6".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn iff_up_without_running_still_counts() {
        let up = libc::IFF_UP as libc::c_uint;
        let running = libc::IFF_RUNNING as libc::c_uint;
        let loopback = libc::IFF_LOOPBACK as libc::c_uint;

        assert!(ip::interface_flags_allow_ip(up));
        assert!(ip::interface_flags_allow_ip(up | running));
        assert!(!ip::interface_flags_allow_ip(running));
        assert!(!ip::interface_flags_allow_ip(up | loopback));
    }

    #[test]
    fn network_rate_uses_the_real_elapsed_window() {
        let start = NetworkTotals { up: 100, down: 200 };
        let end = NetworkTotals { up: 300, down: 600 };
        let sample = network_sample(start, end, Duration::from_secs(2));
        assert_eq!(sample.up, 100);
        assert_eq!(sample.down, 200);
        assert_eq!(sample.total_up, 300);
        assert_eq!(sample.total_down, 600);
    }

    #[test]
    fn monitoring_mountpoints_deduplicates_and_keeps_shortest() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("mounts"),
            "/dev/sda1 / ext4 rw 0 0\n/dev/sda1 /data/sub ext4 rw 0 0\n/dev/sr0 /mnt/cdrom iso9660 ro 0 0\n",
        )
        .unwrap();

        let mountpoints = monitoring_mountpoints(directory.path(), "").unwrap();
        assert!(mountpoints.contains(&"/ (ext4)".to_owned()));
        assert!(
            !mountpoints
                .iter()
                .any(|mountpoint| mountpoint.contains("/data"))
        );

        let explicit = monitoring_mountpoints(directory.path(), "/;/data").unwrap();
        assert_eq!(explicit, vec!["/".to_owned(), "/data".to_owned()]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_escapes_are_decoded() {
        let line = "/dev/sda1 /mnt/my\\040disk ext4 rw 0 0";
        let parsed = disk::parse_mount_line(line).unwrap();
        assert_eq!(parsed.mountpoint, "/mnt/my disk");
        assert_eq!(parsed.fstype, "ext4");
    }
}
