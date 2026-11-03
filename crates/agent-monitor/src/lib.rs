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

mod error;
mod options;
mod types;

pub use error::MonitorError;
pub use options::{DEFAULT_PROC_ROOT, HOST_PROC_ENV, MonitorOptions};
pub use types::{
    BasicInfo, CpuInfo, LoadAverage, MemoryInfo, MemoryMode, OsInfo, SwapInfo, SystemSnapshot,
};
