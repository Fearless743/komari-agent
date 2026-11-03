//! Parsers and readers for `/proc` files.
//!
//! All readers take an explicit proc root so container deployments can point
//! them at a mounted host proc filesystem. Parsers are pure functions and are
//! exercised by the fixture-based tests.

use std::fs;
use std::path::Path;

use crate::error::MonitorError;

/// Raw CPU time counters from `/proc/stat`, in jiffies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    fn from_fields(fields: &[u64]) -> Self {
        let field = |index: usize| fields.get(index).copied().unwrap_or(0);
        Self {
            user: field(0),
            nice: field(1),
            system: field(2),
            idle: field(3),
            iowait: field(4),
            irq: field(5),
            softirq: field(6),
            steal: field(7),
        }
    }

    /// Total non-guest CPU time.
    pub fn total(self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    /// Idle time, counting iowait as idle like the Go/gopsutil calculation.
    pub fn idle_total(self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }
}

/// A parsed `/proc/stat` CPU section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcStat {
    /// The aggregate `cpu` line.
    pub aggregate: CpuTimes,
    /// Per-core `cpuN` lines, in file order.
    pub cores: Vec<CpuTimes>,
}

/// CPU usage percentages computed from two `/proc/stat` samples.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuUsage {
    pub total: f64,
    pub per_core: Vec<f64>,
}

/// Parse the CPU lines of `/proc/stat`.
///
/// The aggregate `cpu` line is required; per-core lines are optional. Lines
/// with fewer than four numeric fields are ignored, mirroring the tolerant
/// behaviour of the Go agent.
pub fn parse_proc_stat(content: &str) -> Result<ProcStat, MonitorError> {
    let mut aggregate = None;
    let mut cores = Vec::new();

    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let Some(label) = fields.next() else {
            continue;
        };
        if label != "cpu" && !label.starts_with("cpu") {
            continue;
        }
        if label != "cpu" && !label[3..].chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }

        let values: Vec<u64> = fields
            .filter_map(|field| field.parse::<u64>().ok())
            .collect();
        if values.len() < 4 {
            continue;
        }

        if label == "cpu" {
            aggregate = Some(CpuTimes::from_fields(&values));
        } else {
            cores.push(CpuTimes::from_fields(&values));
        }
    }

    let Some(aggregate) = aggregate else {
        return Err(MonitorError::parse(
            "proc/stat",
            "missing aggregate cpu line",
        ));
    };

    Ok(ProcStat { aggregate, cores })
}

/// Read and parse `<proc_root>/stat`.
pub fn read_proc_stat(proc_root: &Path) -> Result<ProcStat, MonitorError> {
    let path = proc_root.join("stat");
    let content = fs::read_to_string(&path).map_err(|source| MonitorError::io(&path, source))?;
    parse_proc_stat(&content)
}

/// Raw memory counters from `/proc/meminfo`, in bytes.
///
/// Field set and byte conversion match Go's `monitoring.ProcMemInfo`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcMemInfo {
    pub mem_total: u64,
    pub mem_free: u64,
    pub mem_available: u64,
    pub buffers: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub swap_cached: u64,
    pub shmem: u64,
    pub s_reclaimable: u64,
    pub zswap: u64,
    pub zswapped: u64,
}

impl ProcMemInfo {
    /// Available memory reported by the kernel, falling back to
    /// `total - free - buffers - cached` on old kernels without `MemAvailable`.
    pub fn available(&self) -> u64 {
        if self.mem_available > 0 {
            self.mem_available.min(self.mem_total)
        } else {
            self.mem_total.saturating_sub(
                self.mem_free
                    .saturating_add(self.buffers)
                    .saturating_add(self.cached),
            )
        }
    }

    /// htop-like used memory, matching Go's `GetMemHtopLike`:
    /// `total - (free + cached + sreclaimable + buffers) + shmem`.
    ///
    /// If the reclaimable sum is larger than the total (possible on some
    /// container/initramfs setups), it falls back to `total - free` like Go
    /// instead of underflowing.
    pub fn htop_used(&self) -> u64 {
        let reclaimable = self
            .mem_free
            .saturating_add(self.cached)
            .saturating_add(self.s_reclaimable)
            .saturating_add(self.buffers);
        let base = if self.mem_total >= reclaimable {
            self.mem_total - reclaimable
        } else {
            self.mem_total.saturating_sub(self.mem_free)
        };
        base.saturating_add(self.shmem)
    }
}

/// Parse `/proc/meminfo`.
///
/// Unknown keys and malformed values are ignored, matching the Go parser. All
/// values are in kB and converted to bytes.
pub fn parse_meminfo(content: &str) -> ProcMemInfo {
    let mut info = ProcMemInfo::default();

    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let Some(key) = fields.next() else {
            continue;
        };
        let Some(value) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            continue;
        };
        let value = value.saturating_mul(1024);
        let key = key.trim_end_matches(':');

        match key {
            "MemTotal" => info.mem_total = value,
            "MemFree" => info.mem_free = value,
            "MemAvailable" => info.mem_available = value,
            "Buffers" => info.buffers = value,
            "Cached" => info.cached = value,
            "SwapTotal" => info.swap_total = value,
            "SwapFree" => info.swap_free = value,
            "SwapCached" => info.swap_cached = value,
            "Shmem" => info.shmem = value,
            "SReclaimable" => info.s_reclaimable = value,
            "Zswap" => info.zswap = value,
            "Zswapped" => info.zswapped = value,
            _ => {}
        }
    }

    info
}

/// Read and parse `<proc_root>/meminfo`.
pub fn read_meminfo(proc_root: &Path) -> Result<ProcMemInfo, MonitorError> {
    let path = proc_root.join("meminfo");
    let content = fs::read_to_string(&path).map_err(|source| MonitorError::io(&path, source))?;
    Ok(parse_meminfo(&content))
}

/// Compute usage percentages between two samples.
///
/// Counters that moved backwards (container migration, kernel counter reset)
/// are clamped to zero instead of producing a negative or out-of-range value.
pub fn usage_between(previous: &ProcStat, current: &ProcStat) -> CpuUsage {
    CpuUsage {
        total: usage_percent(previous.aggregate, current.aggregate),
        per_core: previous
            .cores
            .iter()
            .zip(current.cores.iter())
            .map(|(previous, current)| usage_percent(*previous, *current))
            .collect(),
    }
}

fn usage_percent(previous: CpuTimes, current: CpuTimes) -> f64 {
    let total_delta = current.total().saturating_sub(previous.total());
    if total_delta == 0 {
        return 0.0;
    }

    let idle_delta = current
        .idle_total()
        .saturating_sub(previous.idle_total())
        .min(total_delta);
    let busy_delta = total_delta - idle_delta;

    (busy_delta as f64 * 100.0 / total_delta as f64).clamp(0.0, 100.0)
}
