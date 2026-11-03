//! Disk usage collection.
//!
//! Mirrors `monitoring/unit/disk.go`: either sum an explicit list of mount
//! points (`include_mountpoints`, semicolon separated) or enumerate the
//! partitions, filter pseudo filesystems and aggregate physical devices,
//! deduplicating by device (ZFS datasets collapse to their pool).
//!
//! On Linux the partition table comes from the injected proc root
//! (`<proc_root>/mounts`, falling back to `<proc_root>/self/mounts`) and usage
//! is read with `statvfs`. Other platforms use `sysinfo`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::options::MonitorOptions;
use crate::types::DiskInfo;

/// One mount table entry (`/proc/mounts` style).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartitionStat {
    pub device: String,
    pub mountpoint: String,
    pub fstype: String,
    pub opts: Vec<String>,
}

/// Usage of one mount point in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    pub used: u64,
}

/// Parse a mount table.
///
/// Fields are whitespace separated; the kernel escapes spaces and tabs as
/// `\040` / `\011`, which are unescaped like gopsutil's mount parser.
pub fn parse_mounts(content: &str) -> Vec<PartitionStat> {
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = unescape_mount_field(fields.next()?);
            let mountpoint = unescape_mount_field(fields.next()?);
            let fstype = unescape_mount_field(fields.next()?);
            let opts = fields
                .next()
                .map(|opts| opts.split(',').map(str::to_owned).collect())
                .unwrap_or_default();
            if device.is_empty() || mountpoint.is_empty() {
                return None;
            }
            Some(PartitionStat {
                device,
                mountpoint,
                fstype,
                opts,
            })
        })
        .collect()
}

/// Parse a `/proc/1/mountinfo` table.
///
/// Fields are `id parent major:minor root mountpoint opts [optional...] - fstype source superopts`;
/// optional fields can contain escaped spaces but never a literal `-`.
pub fn parse_mountinfo(content: &str) -> Vec<PartitionStat> {
    content
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let separator = fields.iter().position(|field| *field == "-")?;
            if separator < 6 || separator + 2 >= fields.len() {
                return None;
            }

            let mountpoint = unescape_mount_field(fields[4]);
            let opts = fields[5].split(',').map(str::to_owned).collect::<Vec<_>>();
            let fstype = unescape_mount_field(fields[separator + 1]);
            let device = unescape_mount_field(fields[separator + 2]);
            if mountpoint.is_empty() {
                return None;
            }

            Some(PartitionStat {
                device,
                mountpoint,
                fstype,
                opts,
            })
        })
        .collect()
}

fn unescape_mount_field(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_owned();
    }
    let bytes = field.as_bytes();
    let mut out = String::with_capacity(field.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let octal = &field[index + 1..index + 4];
            if let Ok(value) = u8::from_str_radix(octal, 8) {
                out.push(char::from(value));
                index += 4;
                continue;
            }
        }
        out.push(char::from(bytes[index]));
        index += 1;
    }
    out
}

/// Parse the semicolon separated mount point include list.
pub fn parse_include_mountpoints(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|mountpoint| !mountpoint.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Go's `isPhysicalDisk`: exclude pseudo filesystems, temporary mounts and
/// network drives. The root mount point is always kept so LXC/loop based root
/// filesystems are still reported.
pub fn is_physical_disk(part: &PartitionStat) -> bool {
    if part.mountpoint == "/" {
        return true;
    }

    let mountpoint = part.mountpoint.to_ascii_lowercase();
    const EXCLUDED_MOUNTPOINTS: [&str; 12] = [
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
    if EXCLUDED_MOUNTPOINTS
        .iter()
        .any(|excluded| mountpoint == *excluded || mountpoint.starts_with(excluded))
    {
        return false;
    }

    let fstype = part.fstype.to_ascii_lowercase();

    // autofs triggers are not physical unless backed by /dev.
    if fstype == "autofs" && !part.device.starts_with("/dev/") {
        return false;
    }

    // ntfs-3g mounts are physical even though the fstype is fuseblk.
    if fstype == "fuseblk" {
        return true;
    }

    const EXCLUDED_FSTYPES: [&str; 19] = [
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
    if EXCLUDED_FSTYPES
        .iter()
        .any(|excluded| fstype == *excluded || fstype.starts_with(excluded))
    {
        return false;
    }

    let opts = part.opts.join(",").to_ascii_lowercase();
    if opts.contains("remote") || opts.contains("network") {
        return false;
    }

    if part.device.starts_with("/dev/loop") {
        return false;
    }

    true
}

/// Device identity used for deduplication. ZFS datasets collapse to the pool
/// name (`pool/dataset` -> `pool`).
fn device_id(part: &PartitionStat) -> String {
    let mut device = part.device.clone();
    if part.fstype.eq_ignore_ascii_case("zfs")
        && let Some(index) = device.find('/')
    {
        device.truncate(index);
    }
    device
}

/// Aggregate physical partitions: one entry per device, keeping the mount with
/// the largest total (which handles quotas and bind mounts).
pub fn aggregate_device_usage(entries: &[(PartitionStat, DiskUsage)]) -> DiskInfo {
    let mut devices: BTreeMap<String, DiskUsage> = BTreeMap::new();
    for (part, usage) in entries {
        let id = device_id(part);
        match devices.get(&id) {
            Some(existing) if existing.total >= usage.total => {}
            _ => {
                devices.insert(id, *usage);
            }
        }
    }

    devices
        .values()
        .fold(DiskInfo::default(), |mut total, usage| {
            total.total = total.total.saturating_add(usage.total);
            total.used = total.used.saturating_add(usage.used);
            total
        })
}

/// Sum an explicit list of included mount points. Errors are skipped, matching
/// Go's `continue` on `disk.Usage` failure.
pub fn sum_included_usage(entries: &[DiskUsage]) -> DiskInfo {
    entries
        .iter()
        .fold(DiskInfo::default(), |mut total, usage| {
            total.total = total.total.saturating_add(usage.total);
            total.used = total.used.saturating_add(usage.used);
            total
        })
}

/// Collect disk totals.
pub(crate) fn collect(options: &MonitorOptions) -> DiskInfo {
    let included = parse_include_mountpoints(&options.include_mountpoints);

    #[cfg(target_os = "linux")]
    {
        // Go calls disk.Partitions first: when the mount table cannot be read
        // the whole collection (including the explicit include list) is zero.
        let Some(parts) = read_linux_mounts(options) else {
            return DiskInfo::default();
        };

        // The branch is selected by the *raw* configuration string, matching
        // Go's `flags.IncludeMountpoints != ""`. A value such as " " or ";"
        // therefore enters the include branch and sums nothing.
        if !options.include_mountpoints.is_empty() {
            let usages: Vec<DiskUsage> = included
                .iter()
                .filter_map(|mountpoint| usage_for_path(Path::new(mountpoint)))
                .collect();
            return sum_included_usage(&usages);
        }

        let entries: Vec<(PartitionStat, DiskUsage)> = parts
            .into_iter()
            .filter(is_physical_disk)
            .filter_map(|part| {
                let usage = usage_for_path(Path::new(&part.mountpoint))?;
                Some((part, usage))
            })
            .collect();
        aggregate_device_usage(&entries)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let parts = crate::platform::fallback::disk_partitions();
        if !options.include_mountpoints.is_empty() {
            let usages: Vec<DiskUsage> = included
                .iter()
                .filter_map(|mountpoint| {
                    usage_for_path(Path::new(mountpoint)).or_else(|| {
                        parts
                            .iter()
                            .find(|(part, _)| part.mountpoint == *mountpoint)
                            .map(|(_, usage)| *usage)
                    })
                })
                .collect();
            return sum_included_usage(&usages);
        }

        let entries: Vec<(PartitionStat, DiskUsage)> = parts
            .into_iter()
            .filter(|(part, _)| is_physical_disk(part))
            .collect();
        aggregate_device_usage(&entries)
    }
}

#[cfg(target_os = "linux")]
fn read_linux_mounts(options: &MonitorOptions) -> Option<Vec<PartitionStat>> {
    // gopsutil reads /proc/1/mountinfo first, then falls back to /proc/mounts.
    let mountinfo = options.proc_path("1/mountinfo");
    if let Ok(content) = std::fs::read_to_string(&mountinfo) {
        let parts = parse_mountinfo(&content);
        if !parts.is_empty() {
            return Some(parts);
        }
    }

    for relative in ["mounts", "self/mounts"] {
        let path = options.proc_path(relative);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let parts = parse_mounts(&content);
            if !parts.is_empty() {
                return Some(parts);
            }
        }
    }
    None
}

/// Read usage for one path.
#[cfg(unix)]
pub(crate) fn usage_for_path(path: &Path) -> Option<DiskUsage> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid NUL terminated string and `stat` is fully
    // initialised by statvfs on success.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }

    let block_size = stat.f_frsize as u64;
    let total = (stat.f_blocks as u64).saturating_mul(block_size);
    let free = (stat.f_bfree as u64).saturating_mul(block_size);
    Some(DiskUsage {
        total,
        used: total.saturating_sub(free),
    })
}

/// Read usage for one path on non-Unix platforms through `sysinfo`.
#[cfg(not(unix))]
pub(crate) fn usage_for_path(path: &Path) -> Option<DiskUsage> {
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .find(|disk| disk.mount_point() == path)
        .map(|disk| DiskUsage {
            total: disk.total_space(),
            used: disk.total_space().saturating_sub(disk.available_space()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(device: &str, mountpoint: &str, fstype: &str, opts: &str) -> PartitionStat {
        PartitionStat {
            device: device.to_owned(),
            mountpoint: mountpoint.to_owned(),
            fstype: fstype.to_owned(),
            opts: opts.split(',').map(str::to_owned).collect(),
        }
    }

    #[test]
    fn parses_mounts_and_unescapes_octal_sequences() {
        let mounts = "\
/dev/sda1 / ext4 rw,relatime 0 0
/dev/sdb1 /mnt/with\\040space xfs rw,noatime 0 0
tmpfs /run tmpfs rw,nosuid 0 0
";
        let parts = parse_mounts(mounts);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].mountpoint, "/mnt/with space");
        assert_eq!(parts[1].opts, vec!["rw", "noatime"]);
    }

    #[test]
    fn parses_mountinfo_with_device_after_separator() {
        let content = "\
36 29 0:32 / /sys rw,nosuid,nodev shared:3 - sysfs sysfs rw
37 29 8:1 / /mnt/with\\040space ext4 rw,relatime shared:7 - ext4 /dev/sda1 rw
";
        let parts = parse_mountinfo(content);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].mountpoint, "/sys");
        assert_eq!(parts[0].fstype, "sysfs");
        assert_eq!(parts[0].device, "sysfs");
        assert_eq!(parts[1].mountpoint, "/mnt/with space");
        assert_eq!(parts[1].fstype, "ext4");
        assert_eq!(parts[1].device, "/dev/sda1");
    }

    #[test]
    fn filters_pseudo_and_virtual_filesystems_like_go() {
        assert!(is_physical_disk(&part("/dev/sda1", "/", "ext4", "rw")));
        assert!(!is_physical_disk(&part("tmpfs", "/tmp", "tmpfs", "rw")));
        assert!(!is_physical_disk(&part("proc", "/proc", "proc", "rw")));
        assert!(!is_physical_disk(&part("sysfs", "/sys", "sysfs", "rw")));
        assert!(!is_physical_disk(&part(
            "/dev/sda1",
            "/var/lib/docker",
            "ext4",
            "rw"
        )));
        assert!(!is_physical_disk(&part(
            "server:/export",
            "/mnt/nfs",
            "nfs",
            "rw"
        )));
        assert!(!is_physical_disk(&part(
            "/dev/loop0",
            "/snap/core",
            "squashfs",
            "rw"
        )));
        assert!(is_physical_disk(&part(
            "/dev/sda2",
            "/mnt/ntfs",
            "fuseblk",
            "rw"
        )));
        assert!(!is_physical_disk(&part(
            "auto",
            "/mnt/auto",
            "autofs",
            "rw"
        )));
        assert!(is_physical_disk(&part(
            "/dev/sda3",
            "/mnt/auto",
            "autofs",
            "rw"
        )));
        assert!(!is_physical_disk(&part(
            "/dev/sdb1",
            "/mnt/share",
            "ext4",
            "rw,remote"
        )));
    }

    #[test]
    fn deduplicates_devices_keeping_largest_total_and_collapses_zfs_pool() {
        let entries = vec![
            (
                part("/dev/sda1", "/", "ext4", "rw"),
                DiskUsage {
                    total: 1000,
                    used: 400,
                },
            ),
            (
                part("/dev/sda1", "/mnt/bind", "ext4", "rw"),
                DiskUsage {
                    total: 2000,
                    used: 800,
                },
            ),
            (
                part("pool/dataset-a", "/a", "zfs", "rw"),
                DiskUsage {
                    total: 500,
                    used: 100,
                },
            ),
            (
                part("pool/dataset-b", "/b", "zfs", "rw"),
                DiskUsage {
                    total: 700,
                    used: 200,
                },
            ),
        ];

        let disk = aggregate_device_usage(&entries);
        assert_eq!(disk.total, 2700);
        assert_eq!(disk.used, 1000);
    }

    #[test]
    fn include_list_parsing_trims_and_skips_empty_entries() {
        assert_eq!(
            parse_include_mountpoints(" / ; /data ;; /mnt/x "),
            vec!["/", "/data", "/mnt/x"]
        );
        assert!(parse_include_mountpoints("   ").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn usage_for_single_mount_point_is_non_negative() {
        let usage = usage_for_path(Path::new("/")).expect("root is readable");
        assert!(usage.total > 0);
        assert!(usage.used <= usage.total);
    }
}
