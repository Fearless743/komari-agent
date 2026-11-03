//! Public data types produced by the collector.
//!
//! Field names mirror the Go agent so that the protocol layer (A04/A05) can map
//! them without guessing:
//!
//! - [`CpuInfo`] uses the JSON keys of `monitoring.CpuInfo`.
//! - [`MemoryInfo`] / [`SwapInfo`] / [`LoadAverage`] use the keys of the
//!   `ram`, `swap` and `load` objects built by `monitoring.GenerateReport`.
//! - [`BasicInfo`] uses the keys uploaded by `server.uploadBasicInfo`.

use serde::{Deserialize, Serialize};

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

/// Disk usage totals, matching Go's `monitoring.DiskInfo` / the `disk` section
/// of `GenerateReport`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    pub total: u64,
    pub used: u64,
}

/// Per-interface cumulative counters read from `/proc/net/dev` (or `sysinfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InterfaceCounters {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
}

/// Network transfer rates and cumulative counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkSpeed {
    /// Bytes per second since the previous sample.
    pub up: u64,
    /// Bytes per second since the previous sample.
    pub down: u64,
    /// Cumulative transmitted bytes of the selected interfaces.
    pub total_up: u64,
    /// Cumulative received bytes of the selected interfaces.
    pub total_down: u64,
}

/// `network` object of the periodic report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkReport {
    /// Upload rate, bytes per second.
    pub up: u64,
    /// Download rate, bytes per second.
    pub down: u64,
    /// Cumulative upload bytes (`totalUp` on the wire).
    #[serde(rename = "totalUp")]
    pub total_up: u64,
    /// Cumulative download bytes (`totalDown` on the wire).
    #[serde(rename = "totalDown")]
    pub total_down: u64,
}

impl From<NetworkSpeed> for NetworkReport {
    fn from(speed: NetworkSpeed) -> Self {
        Self {
            up: speed.up,
            down: speed.down,
            total_up: speed.total_up,
            total_down: speed.total_down,
        }
    }
}

/// Socket counts from `/proc/net/{tcp,tcp6,udp,udp6}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionsCount {
    pub tcp: i64,
    pub udp: i64,
}

/// `connections` object of the periodic report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionsReport {
    pub tcp: i64,
    pub udp: i64,
}

impl From<ConnectionsCount> for ConnectionsReport {
    fn from(count: ConnectionsCount) -> Self {
        Self {
            tcp: count.tcp,
            udp: count.udp,
        }
    }
}

/// One GPU described by Go's `monitoring.DetailedGPUInfo` / `GPUInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub memory_total: u64,
    pub memory_used: u64,
    pub utilization: f64,
    pub temperature: u64,
}

/// `gpu` object of the periodic report.
///
/// Go emits two shapes:
///
/// * detailed: `{"count":N,"average_usage":X,"detailed_info":[...]}`
/// * models only: `{"models":["..."]}`
///
/// Optional fields make both shapes expressible without losing a present zero
/// value such as `average_usage: 0.0`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_usage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detailed_info: Option<Vec<GpuInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
}

impl GpuReport {
    /// Build the detailed shape. Returns `None` when no GPU was reported, so
    /// the report omits `gpu` instead of emitting a divide-by-zero average.
    pub fn detailed(devices: Vec<GpuInfo>) -> Option<Self> {
        if devices.is_empty() {
            return None;
        }
        let average_usage =
            devices.iter().map(|gpu| gpu.utilization).sum::<f64>() / devices.len() as f64;
        Some(Self {
            count: Some(i64::try_from(devices.len()).unwrap_or(i64::MAX)),
            average_usage: Some(average_usage),
            detailed_info: Some(devices),
            models: None,
        })
    }

    /// Build the model-only fallback shape.
    pub fn models(models: Vec<String>) -> Option<Self> {
        let models: Vec<String> = models
            .into_iter()
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty())
            .collect();
        if models.is_empty() {
            return None;
        }
        Some(Self {
            count: None,
            average_usage: None,
            detailed_info: None,
            models: Some(models),
        })
    }
}

/// Metrics collected for the periodic report.
///
/// This serializes with the same wire shape as Go's
/// `monitoring.GenerateReport()`:
///
/// ```json
/// {
///   "cpu": {"usage": 0.001},
///   "ram": {"total": 0, "used": 0},
///   "swap": {"total": 0, "used": 0},
///   "load": {"load1": 0.0, "load5": 0.0, "load15": 0.0},
///   "disk": {"total": 0, "used": 0},
///   "network": {"up": 0, "down": 0, "totalUp": 0, "totalDown": 0},
///   "connections": {"tcp": 0, "udp": 0},
///   "uptime": 0,
///   "process": 0,
///   "message": ""
/// }
/// ```
///
/// `gpu` is absent unless [`MonitorOptions::enable_gpu`](crate::MonitorOptions::enable_gpu)
/// is set and a GPU was detected. `Collector::collect_report_metrics()` does
/// not probe OS or virtualization, so it is safe to call every reporting
/// interval.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReportMetrics {
    pub cpu: CpuReport,
    pub ram: RamReport,
    pub swap: RamReport,
    pub load: LoadAverage,
    pub disk: DiskInfo,
    pub network: NetworkReport,
    pub connections: ConnectionsReport,
    /// Only present when detailed GPU probing produced a result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuReport>,
    pub uptime: u64,
    /// Number of processes, matching Go's `int` (`i64` here).
    pub process: i64,
    /// Diagnostics accumulated while collecting, mirroring Go's `message`.
    pub message: String,
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
    pub disk_total: u64,
    pub gpu_name: String,
    pub virtualization: String,
}

/// One coherent snapshot of the metrics owned by this crate.
///
/// This is a convenience aggregate for diagnostics and tests, **not** the
/// protocol wire format: [`CpuInfo`] serializes Go's `monitoring.CpuInfo` keys
/// (`cpu_usage`, `cpu_cores`, ...), while the periodic report uses
/// [`ReportMetrics`] (`cpu.usage`). IP addresses are added by a later task.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemSnapshot {
    pub cpu: CpuInfo,
    /// Serialized as `ram`, matching `common.Report`.
    #[serde(rename = "ram")]
    pub memory: MemoryInfo,
    pub swap: SwapInfo,
    pub load: LoadAverage,
    pub disk: DiskInfo,
    pub network: NetworkReport,
    pub connections: ConnectionsReport,
    pub process: i64,
    /// Basic GPU model string (`None` when undetectable), matching Go's
    /// `GpuName()`.
    pub gpu_name: String,
    /// Detailed/model-only GPU report when `enable_gpu` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuReport>,
    pub os: OsInfo,
    /// Seconds since boot.
    pub uptime: u64,
    /// Hypervisor/container name, or an empty string when detection failed.
    pub virtualization: String,
}
