//! `check-mem` and `list-disk` subcommands.
//!
//! Output mirrors the Go commands closely enough for operators: the same
//! headings, byte/MiB values and tab-aligned partition table.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use agent_client::{list_partitions, monitoring_mountpoints};
use agent_monitor::{MemoryMode, read_meminfo};

/// Run `check-mem` against the configured proc root.
///
/// `include_cache` and `report_raw_used` select the same accounting mode as the
/// collector, so the final block mirrors Go's `monitoring.Ram()`.
pub fn check_mem(
    proc_root: &Path,
    include_cache: bool,
    report_raw_used: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    writeln!(out, "--- Memory Check ---")?;

    let info = read_meminfo(proc_root).ok();
    if let Some(info) = &info {
        writeln!(out, "--- /proc/meminfo ---")?;
        write_mib(out, "MemTotal", info.mem_total)?;
        write_mib(out, "MemFree", info.mem_free)?;
        write_mib(out, "MemAvailable", info.available())?;
        write_mib(out, "Buffers", info.buffers)?;
        write_mib(out, "Cached", info.cached)?;
        write_mib(out, "SwapTotal", info.swap_total)?;
        write_mib(out, "SwapFree", info.swap_free)?;
        write_mib(out, "SwapCached", info.swap_cached)?;
        write_mib(out, "Shmem", info.shmem)?;
        write_mib(out, "SReclaimable", info.s_reclaimable)?;
        write_mib(out, "Zswap", info.zswap)?;
        write_mib(out, "Zswapped", info.zswapped)?;
        writeln!(out, "---------------------")?;
    } else {
        writeln!(out, "failed to read {}/meminfo", proc_root.display())?;
    }

    // Go prints these three modes unconditionally, even when the raw meminfo
    // read fails (the htop-like values then stay zero).
    let (htop_total, htop_used) = info
        .as_ref()
        .map(|info| (info.mem_total, info.htop_used()))
        .unwrap_or((0, 0));
    write_mode(out, MemoryMode::HtopLike, htop_total, htop_used)?;

    let (gopsutil_total, gopsutil_used) = info
        .as_ref()
        .map(|info| {
            (
                info.mem_total,
                info.mem_total.saturating_sub(info.available()),
            )
        })
        .unwrap_or((0, 0));
    write_mode(out, MemoryMode::Gopsutil, gopsutil_total, gopsutil_used)?;

    let (call_free_total, call_free_used) = call_free();
    write_mode_str(out, "callFree", call_free_total, call_free_used)?;

    writeln!(out, "--- Current Configured ---")?;
    if let Some(info) = &info {
        let (mode, used) = if include_cache {
            (
                MemoryMode::IncludeCache,
                info.mem_total.saturating_sub(info.mem_free),
            )
        } else if report_raw_used {
            (MemoryMode::HtopLike, info.htop_used())
        } else {
            #[cfg(target_os = "linux")]
            let mode = MemoryMode::HtopLike;
            #[cfg(not(target_os = "linux"))]
            let mode = MemoryMode::Gopsutil;
            (mode, info.mem_total.saturating_sub(info.available()))
        };
        write_mode(out, mode, info.mem_total, used)?;
    }
    Ok(())
}

fn write_mib(out: &mut impl Write, label: &str, bytes: u64) -> io::Result<()> {
    writeln!(out, "{label}: {} MiB", bytes / 1024 / 1024)
}

fn write_mode(out: &mut impl Write, mode: MemoryMode, total: u64, used: u64) -> io::Result<()> {
    write_mode_str(out, mode.as_str(), total, used)
}

fn write_mode_str(out: &mut impl Write, mode: &str, total: u64, used: u64) -> io::Result<()> {
    writeln!(
        out,
        "[{}] Total: {} bytes ({} MiB), Used: {} bytes ({} MiB)",
        mode,
        total,
        total / 1024 / 1024,
        used,
        used / 1024 / 1024
    )
}

/// Port of Go's `monitoring.CallFree()`: parse `free -b` on Linux/FreeBSD.
fn call_free() -> (u64, u64) {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let Ok(output) = std::process::Command::new("free").arg("-b").output() else {
            return (0, 0);
        };
        if !output.status.success() {
            return (0, 0);
        }
        parse_free_b(&String::from_utf8_lossy(&output.stdout))
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    (0, 0)
}

fn parse_free_b(output: &str) -> (u64, u64) {
    for line in output.lines().skip(1) {
        let Some(rest) = line.strip_prefix("Mem:") else {
            continue;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() >= 2 {
            let total = fields[0].parse::<u64>().unwrap_or(0);
            let used = fields[1].parse::<u64>().unwrap_or(0);
            return (total, used);
        }
        break;
    }
    (0, 0)
}

/// Run `list-disk` against the configured proc root.
pub fn list_disk(
    proc_root: &Path,
    include_mountpoints: &str,
    out: &mut impl Write,
) -> io::Result<()> {
    let partitions = list_partitions(proc_root).unwrap_or_default();

    writeln!(out, "All Disk Partitions:")?;
    writeln!(out, "Mountpoint\tFstype")?;
    for partition in &partitions {
        writeln!(out, "{}\t{}", partition.mountpoint, partition.fstype)?;
    }

    let monitored = monitoring_mountpoints(proc_root, include_mountpoints).unwrap_or_default();
    writeln!(out, "Monitoring Mountpoints: [{}]", monitored.join(" "))?;
    Ok(())
}

/// Resolve the proc root from `HOST_PROC`/configuration.
pub fn proc_root(host_proc: &str) -> PathBuf {
    let trimmed = host_proc.trim();
    if trimmed.is_empty() {
        PathBuf::from("/proc")
    } else {
        PathBuf::from(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_mem_reads_an_injected_proc_root() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("meminfo"),
            "MemTotal:       2048 kB\nMemFree:         512 kB\nMemAvailable:   1024 kB\nSwapTotal:      4096 kB\nSwapFree:       2048 kB\nCached:          256 kB\n",
        )
        .unwrap();

        let mut output = Vec::new();
        check_mem(directory.path(), false, false, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("--- Memory Check ---"));
        assert!(text.contains("MemTotal: 2 MiB"));
        assert!(text.contains("MemAvailable: 1 MiB"));
        assert!(text.contains("[htoplike]"));
        assert!(text.contains("[callFree]"));
        assert!(text.contains("--- Current Configured ---"));
    }

    #[test]
    fn check_mem_prints_modes_even_without_meminfo() {
        let directory = tempfile::tempdir().unwrap();
        let mut output = Vec::new();
        check_mem(directory.path(), false, false, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(!text.contains("--- Memory Modes ---"));
        assert!(text.contains("[htoplike] Total: 0 bytes"));
        assert!(text.contains("[gopsutil] Total: 0 bytes"));
        assert!(text.contains("[callFree]"));
        assert!(text.contains("--- Current Configured ---"));
    }

    #[test]
    fn call_free_parses_the_mem_line() {
        let output = "               total        used        free\nMem:     16384000000  4096000000  8192000000\nSwap:             0           0           0\n";
        assert_eq!(parse_free_b(output), (16384000000, 4096000000));
        assert_eq!(parse_free_b(""), (0, 0));
    }

    #[test]
    fn list_disk_prints_partitions_and_monitored_mounts() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("mounts"),
            "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /mnt/data\\040disk fuseblk rw 0 0\n",
        )
        .unwrap();

        let mut output = Vec::new();
        list_disk(directory.path(), "/;/mnt/data disk", &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("All Disk Partitions:"));
        assert!(text.contains("/mnt/data disk\tfuseblk"));
        assert!(text.contains("Monitoring Mountpoints: [/ /mnt/data disk]"));
    }

    #[test]
    fn default_proc_root_matches_go() {
        assert_eq!(proc_root(""), PathBuf::from("/proc"));
        assert_eq!(proc_root("/host/proc"), PathBuf::from("/host/proc"));
    }
}
