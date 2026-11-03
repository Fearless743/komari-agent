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
