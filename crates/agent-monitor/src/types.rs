//! Public data types produced by the collector.
//!
//! Field names mirror the Go agent so that the protocol layer (A04/A05) can map
//! them without guessing:
//!
//! - [`CpuInfo`] uses the JSON keys of `monitoring.CpuInfo`.
//! - [`MemoryInfo`] / [`SwapInfo`] / [`LoadAverage`] use the keys of the
//!   `ram`, `swap` and `load` objects built by `monitoring.GenerateReport`.
//! - [`BasicInfo`] uses the keys uploaded by `server.uploadBasicInfo`.

use serde::Serialize;

/// CPU information, mirroring Go's `monitoring.CpuInfo`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuInfo {
    #[serde(rename = "cpu_name")]
    pub name: String,
    #[serde(rename = "cpu_architecture")]
    pub arch: String,
    #[serde(rename = "cpu_cores")]
    pub cores: u32,
    /// Physical core count, `0` when unknown, matching the Go agent's
    /// backwards-compatible default.
    #[serde(rename = "cpu_physical_cores")]
    pub physical_cores: u32,
    #[serde(rename = "cpu_usage")]
    pub usage: f64,
    /// Optional per-core usage percentages, ordered like `/proc/stat`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_core_usage: Vec<f64>,
}

impl CpuInfo {
    /// Go-compatible defaults: name `Unknown`, one logical core, no usage.
    pub fn unknown(arch: impl Into<String>) -> Self {
        Self {
            name: "Unknown".to_owned(),
            arch: arch.into(),
            cores: 1,
            physical_cores: 0,
            usage: 0.0,
            per_core_usage: Vec::new(),
        }
    }
}

/// Which memory accounting model produced [`MemoryInfo::used`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryMode {
    /// `total - (free + cached + sreclaimable + buffers) + shmem`
    /// (Go `GetMemHtopLike`, also used for `memory_report_raw_used`).
    #[serde(rename = "htoplike")]
    HtopLike,
    /// `total - available`, matching gopsutil's `VirtualMemory`.
    #[serde(rename = "gopsutil")]
    Gopsutil,
    /// `total - free`, excluding cache from the reported usage
    /// (Go `memory_include_cache`).
    #[serde(rename = "includeCache")]
    IncludeCache,
}

impl MemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HtopLike => "htoplike",
            Self::Gopsutil => "gopsutil",
            Self::IncludeCache => "includeCache",
        }
    }
}

/// RAM information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub free: u64,
    /// Accounting model; intentionally not serialized because Go's `RamInfo`
    /// does not expose it in reports.
    #[serde(skip)]
    pub mode: MemoryMode,
}

impl Default for MemoryInfo {
    fn default() -> Self {
        Self {
            total: 0,
            used: 0,
            available: 0,
            free: 0,
            mode: MemoryMode::HtopLike,
        }
    }
}

/// Swap information. Go reports only totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SwapInfo {
    pub total: u64,
    pub used: u64,
}

/// Load average over 1, 5 and 15 minutes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct LoadAverage {
    #[serde(rename = "load1")]
    pub load1: f64,
    #[serde(rename = "load5")]
    pub load5: f64,
    #[serde(rename = "load15")]
    pub load15: f64,
}

/// CPU object of the periodic report (`cpu.usage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct CpuReport {
    pub usage: f64,
}

/// Total/used pair used by the `ram` and `swap` report objects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RamReport {
    pub total: u64,
    pub used: u64,
}

/// Metrics collected for the periodic report.
///
/// This is the A02 subset of Go's `monitoring.GenerateReport()` and serializes
/// with the same wire shape for the fields it owns:
///
/// ```json
/// {
///   "cpu": {"usage": 0.001},
///   "ram": {"total": 0, "used": 0},
///   "swap": {"total": 0, "used": 0},
///   "load": {"load1": 0.0, "load5": 0.0, "load15": 0.0},
///   "uptime": 0
/// }
/// ```
///
/// Disk, network, GPU, connections and process counts are added by later
/// tasks. `Collector::collect_report_metrics()` does not probe OS or
/// virtualization, so it is safe to call every reporting interval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ReportMetrics {
    pub cpu: CpuReport,
    pub ram: RamReport,
    pub swap: RamReport,
    pub load: LoadAverage,
    pub uptime: u64,
}

/// Operating system information.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OsInfo {
    /// Distribution name (`PRETTY_NAME` from `os-release` on Linux).
    #[serde(rename = "os")]
    pub name: String,
    pub kernel_version: String,
    pub arch: String,
}

/// Fields uploaded through the agent's basic info request.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BasicInfo {
    pub cpu_name: String,
    pub cpu_cores: u32,
    pub cpu_physical_cores: u32,
    pub arch: String,
    pub os: String,
    pub kernel_version: String,
    pub mem_total: u64,
    pub swap_total: u64,
    pub virtualization: String,
}

/// One coherent snapshot of the metrics owned by this crate.
///
/// This is a convenience aggregate for diagnostics and tests, **not** the
/// protocol wire format: [`CpuInfo`] serializes Go's `monitoring.CpuInfo` keys
/// (`cpu_usage`, `cpu_cores`, ...), while the periodic report uses
/// [`ReportMetrics`] (`cpu.usage`). Disk, network and GPU collection live in
/// A03; IP addresses and process counts are added by later tasks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemSnapshot {
    pub cpu: CpuInfo,
    /// Serialized as `ram`, matching `common.Report`.
    #[serde(rename = "ram")]
    pub memory: MemoryInfo,
    pub swap: SwapInfo,
    pub load: LoadAverage,
    pub os: OsInfo,
    /// Seconds since boot.
    pub uptime: u64,
    /// Hypervisor/container name, or an empty string when detection failed.
    pub virtualization: String,
}
