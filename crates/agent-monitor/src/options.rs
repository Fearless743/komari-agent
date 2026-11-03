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

/// Environment variable used by gopsutil to relocate `/sys`.
pub const HOST_SYS_ENV: &str = "HOST_SYS";

/// Default proc filesystem mount point.
pub const DEFAULT_PROC_ROOT: &str = "/proc";

/// Default sys filesystem mount point.
pub const DEFAULT_SYS_ROOT: &str = "/sys";

/// Options controlling how basic system metrics are collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOptions {
    /// Root of the proc filesystem. Container deployments mount the host proc
    /// filesystem somewhere else (for example `/host/proc`) and inject it via
    /// `HOST_PROC`; every `/proc` read goes through this root.
    pub proc_root: PathBuf,
    /// Root of the sys filesystem, used for CPU topology and DMI probing.
    /// Container deployments may inject `HOST_SYS`.
    pub sys_root: PathBuf,
    /// Filesystem root used for distribution markers such as
    /// `/etc/os-release`, `/usr/trim` and `/etc/synoinfo.conf`. Defaults to `/`.
    pub root: PathBuf,
    /// Report memory usage including page cache and buffers
    /// (`total - free`), matching Go's `MemoryIncludeCache`.
    pub memory_include_cache: bool,
    /// Report the raw/htop-like used memory value, matching Go's
    /// `MemoryReportRawUsed`.
    pub memory_report_raw_used: bool,
    /// When true, a failed proc read returns empty/unknown values instead of
    /// falling back to the local `sysinfo` view. [`MonitorOptions::from_config`]
    /// enables this automatically when `HOST_PROC` is configured.
    pub strict_proc: bool,
    /// Interval between the two CPU samples when no previous sample is cached.
    /// The Go agent uses one second.
    pub cpu_sample_interval: Duration,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from(DEFAULT_PROC_ROOT),
            sys_root: PathBuf::from(DEFAULT_SYS_ROOT),
            root: PathBuf::from("/"),
            memory_include_cache: false,
            memory_report_raw_used: false,
            strict_proc: false,
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
    pub fn from_config(
        host_proc: &str,
        memory_include_cache: bool,
        memory_report_raw_used: bool,
    ) -> Self {
        let mut options = Self::default();
        let trimmed = host_proc.trim();
        if !trimmed.is_empty() {
            options.proc_root = PathBuf::from(trimmed);
            // A configured host proc root is authoritative: do not silently
            // substitute the container's own metrics when it is unreadable.
            options.strict_proc = true;
        }
        options.memory_include_cache = memory_include_cache;
        options.memory_report_raw_used = memory_report_raw_used;
        options
    }

    /// Build options using an injected environment lookup.
    ///
    /// `HOST_PROC` and `HOST_SYS` are read here; memory reporting flags come
    /// from the configuration layer, which already applies the Go precedence
    /// rules. Tests can pass a closure instead of mutating the process
    /// environment.
    pub fn from_env_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut options = Self::default();
        if let Some(value) = lookup(HOST_PROC_ENV).filter(|value| !value.trim().is_empty()) {
            options.proc_root = PathBuf::from(value.trim());
        }
        if let Some(value) = lookup(HOST_SYS_ENV).filter(|value| !value.trim().is_empty()) {
            options.sys_root = PathBuf::from(value.trim());
        }
        options
    }

    /// Override the proc filesystem root.
    pub fn with_proc_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.proc_root = root.into();
        self
    }

    /// Override the sys filesystem root.
    pub fn with_sys_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.sys_root = root.into();
        self
    }

    /// Override the filesystem root used for distribution markers.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    /// Override the memory reporting mode flags.
    pub fn with_memory_modes(mut self, include_cache: bool, report_raw_used: bool) -> Self {
        self.memory_include_cache = include_cache;
        self.memory_report_raw_used = report_raw_used;
        self
    }

    /// Override strict proc handling (empty values instead of local fallback).
    pub fn with_strict_proc(mut self, strict: bool) -> Self {
        self.strict_proc = strict;
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

    /// Path of a file relative to the configured sys root.
    pub(crate) fn sys_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.sys_root.join(relative)
    }

    /// Path of a file relative to the configured filesystem root.
    pub(crate) fn root_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }
}
