//! `check-mem` and `list-disk` subcommands.
//!
//! Output mirrors the Go commands closely enough for operators: the same
//! headings, byte/MiB values and tab-aligned partition table.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use agent_client::list_partitions;
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

    if let Ok(info) = read_meminfo(proc_root) {
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

        writeln!(out, "--- Memory Modes ---")?;
        write_mode(out, MemoryMode::HtopLike, info.mem_total, info.htop_used())?;
        write_mode(
            out,
            MemoryMode::Gopsutil,
            info.mem_total,
            info.mem_total.saturating_sub(info.available()),
        )?;
        write_mode(
            out,
            MemoryMode::IncludeCache,
            info.mem_total,
            info.mem_total.saturating_sub(info.mem_free),
        )?;
    } else {
        writeln!(out, "failed to read {}/meminfo", proc_root.display())?;
    }

    writeln!(out, "--- Current Configured ---")?;
    if let Ok(info) = read_meminfo(proc_root) {
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
    writeln!(
        out,
        "[{}] Total: {} bytes ({} MiB), Used: {} bytes ({} MiB)",
        mode.as_str(),
        total,
        total / 1024 / 1024,
        used,
        used / 1024 / 1024
    )
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

    let monitored: Vec<String> = if include_mountpoints.trim().is_empty() {
        partitions
            .iter()
            .map(|partition| partition.mountpoint.clone())
            .collect()
    } else {
        include_mountpoints
            .split(';')
            .map(str::trim)
            .filter(|mountpoint| !mountpoint.is_empty())
            .map(str::to_owned)
            .collect()
    };

    writeln!(out, "Monitoring Mountpoints: {monitored:?}")?;
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
        assert!(text.contains("--- Current Configured ---"));
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
        assert!(text.contains("Monitoring Mountpoints: [\"/\", \"/mnt/data disk\"]"));
    }

    #[test]
    fn default_proc_root_matches_go() {
        assert_eq!(proc_root(""), PathBuf::from("/proc"));
        assert_eq!(proc_root("/host/proc"), PathBuf::from("/host/proc"));
    }
}
