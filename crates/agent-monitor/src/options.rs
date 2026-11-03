//! Collector configuration.
//!
//! The collector never reads the process environment itself. Callers resolve
//! configuration (see `agent-config`) and pass the values in through
//! [`MonitorOptions`]. [`MonitorOptions::from_env_lookup`] exists for standalone
//! use and tests, but still takes an injected lookup function.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable understood by the Go agent for container deployments.
pub const HOST_PROC_ENV: &str = "HOST_PROC";

/// Default proc filesystem mount point.
pub const DEFAULT_PROC_ROOT: &str = "/proc";

/// Options controlling how basic system metrics are collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOptions {
    /// Root of the proc filesystem. Container deployments mount the host proc
    /// filesystem somewhere else (for example `/host/proc`) and inject it via
    /// `HOST_PROC`; every `/proc` read goes through this root.
    pub proc_root: PathBuf,
    /// Report memory usage including page cache and buffers
    /// (`total - free`), matching Go's `MemoryIncludeCache`.
    pub memory_include_cache: bool,
    /// Report the raw/htop-like used memory value, matching Go's
    /// `MemoryReportRawUsed`.
    pub memory_report_raw_used: bool,
    /// Interval between the two CPU samples when no previous sample is cached.
    /// The Go agent uses one second.
    pub cpu_sample_interval: Duration,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from(DEFAULT_PROC_ROOT),
            memory_include_cache: false,
            memory_report_raw_used: false,
            cpu_sample_interval: Duration::from_secs(1),
        }
    }
}

impl MonitorOptions {
    /// Options for the default `/proc` mount.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build options from values already resolved by the configuration layer.
    ///
    /// An empty `host_proc` keeps the default `/proc` root, matching the Go
    /// agent where an empty `HOST_PROC` is ignored.
    pub fn from_config(host_proc: &str, memory_include_cache: bool, memory_report_raw_used: bool) -> Self {
        let mut options = Self::default();
        let trimmed = host_proc.trim();
        if !trimmed.is_empty() {
            options.proc_root = PathBuf::from(trimmed);
        }
        options.memory_include_cache = memory_include_cache;
        options.memory_report_raw_used = memory_report_raw_used;
        options
    }

    /// Build options using an injected environment lookup.
    ///
    /// Only `HOST_PROC` is read here; memory reporting flags come from the
    /// configuration layer, which already applies the Go precedence rules.
    /// Tests can pass a closure instead of mutating the process environment.
    pub fn from_env_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut options = Self::default();
        if let Some(value) = lookup(HOST_PROC_ENV).filter(|value| !value.trim().is_empty()) {
            options.proc_root = PathBuf::from(value.trim());
        }
        options
    }

    /// Override the proc filesystem root.
    pub fn with_proc_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.proc_root = root.into();
        self
    }

    /// Override the memory reporting mode flags.
    pub fn with_memory_modes(mut self, include_cache: bool, report_raw_used: bool) -> Self {
        self.memory_include_cache = include_cache;
        self.memory_report_raw_used = report_raw_used;
        self
    }

    /// Override the CPU sampling interval.
    pub fn with_cpu_sample_interval(mut self, interval: Duration) -> Self {
        self.cpu_sample_interval = interval;
        self
    }

    /// Path of a file relative to the configured proc root.
    pub(crate) fn proc_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.proc_root.join(relative)
    }
}
