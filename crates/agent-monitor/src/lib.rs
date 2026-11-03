//! System metrics collection for the Komari agent.
//!
//! This crate implements CPU, memory, swap, load, disk, network, connections,
//! process count, GPU, OS, uptime and virtualization. IP addresses are owned
//! by a later task.
//!
//! The collector mirrors the field naming and accounting rules of the Go agent
//! (`komari-agent/monitoring`), but keeps the values as typed Rust structs. The
//! wire format is owned by `agent-protocol` (A04).
//!
//! # Proc root injection
//!
//! Every `/proc` read goes through [`MonitorOptions::proc_root`]. Container
//! deployments set `HOST_PROC=/host/proc`; the configuration layer resolves it
//! and passes the value to [`MonitorOptions::from_config`]. The collector itself
//! never reads the process environment.

mod collector;
mod connections;
mod cpu;
mod disk;
mod error;
mod gpu;
mod load;
mod memory;
mod net;
mod netstatic;
mod options;
mod os;
mod platform;
mod process;
mod procfs;
mod types;
mod uptime;
mod virtualization;

pub use collector::Collector;
pub use connections::{count_proc_net, count_proc_net_files};
pub use disk::{
    DiskUsage, PartitionStat, aggregate_device_usage, is_physical_disk, parse_include_mountpoints,
    parse_mountinfo, parse_mounts, sum_included_usage,
};
pub use error::MonitorError;
pub use gpu::name::format_gpu_name_list;
pub use net::{NicFilter, network_speed, parse_nics, parse_proc_net_dev, should_include};
pub use netstatic::{
    DEFAULT_DATA_PRESERVE_DAYS, DEFAULT_DETECT_INTERVAL, DEFAULT_SAVE_INTERVAL, NetStatic,
    NetStaticConfig, NetStaticStore, TrafficData, last_reset_date, safe_delta,
};
pub use options::{
    DEFAULT_PROC_ROOT, DEFAULT_SYS_ROOT, HOST_PROC_ENV, HOST_SYS_ENV, MonitorOptions, PATH_ENV,
};
pub use process::count_proc;
pub use procfs::{
    CpuTimes, CpuUsage, ProcMemInfo, ProcStat, parse_loadavg, parse_meminfo, parse_proc_stat,
    parse_uptime, read_loadavg, read_meminfo, read_proc_stat, read_uptime, usage_between,
};
pub use types::{
    BasicInfo, ConnectionsCount, ConnectionsReport, CpuInfo, CpuReport, DiskInfo, GpuInfo,
    GpuReport, InterfaceCounters, LoadAverage, MemoryInfo, MemoryMode, NetworkReport, NetworkSpeed,
    OsInfo, RamReport, ReportMetrics, SwapInfo, SystemSnapshot,
};
pub use virtualization::detect_virtualization;

/// Collect a snapshot using the default `/proc` root and Go-compatible defaults.
///
/// This is a convenience for callers that do not need stateful CPU sampling;
/// it blocks for up to one second on the first CPU sample, like the Go agent.
pub fn collect() -> SystemSnapshot {
    Collector::new(MonitorOptions::default()).collect()
}

/// Collect the basic information payload with default options.
pub fn collect_basic_info() -> BasicInfo {
    Collector::new(MonitorOptions::default()).collect_basic_info()
}
