//! System metrics collection for the Komari agent.
//!
//! This crate implements the **basic** metrics only: CPU, memory, swap, load,
//! OS, uptime and virtualization. Disk, network, GPU, IP addresses and process
//! counts are implemented in A03 and later tasks.
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
mod cpu;
mod error;
mod load;
mod memory;
mod options;
mod os;
mod platform;
mod procfs;
mod types;
mod uptime;
mod virtualization;

pub use collector::Collector;
pub use error::MonitorError;
pub use options::{DEFAULT_PROC_ROOT, HOST_PROC_ENV, MonitorOptions};
pub use procfs::{
    CpuTimes, CpuUsage, ProcMemInfo, ProcStat, parse_loadavg, parse_meminfo, parse_proc_stat,
    parse_uptime, read_loadavg, read_meminfo, read_proc_stat, read_uptime, usage_between,
};
pub use types::{
    BasicInfo, CpuInfo, LoadAverage, MemoryInfo, MemoryMode, OsInfo, SwapInfo, SystemSnapshot,
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
