//! Collector configuration.
//!
//! The collector does not resolve configuration from the process environment.
//! Callers resolve it (see `agent-config`) and pass values through
//! [`MonitorOptions`]; [`MonitorOptions::from_env_lookup`] exists for standalone
//! use and tests but still takes an injected lookup function.
//!
//! The one process-environment read in the collector is the `PATH` fallback
//! used to locate `pveversion` when [`MonitorOptions::process_path`] is not set.
//! Inject it with [`MonitorOptions::with_process_path`] (or via
//! `from_env_lookup`) to avoid that fallback.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable understood by the Go agent for container deployments.
pub const HOST_PROC_ENV: &str = "HOST_PROC";

/// Environment variable used by gopsutil to relocate `/sys`.
pub const HOST_SYS_ENV: &str = "HOST_SYS";

/// Environment variable used to locate external commands such as `pveversion`.
pub const PATH_ENV: &str = "PATH";

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
    /// Optional `PATH` used to locate `pveversion`. When `None`, the collector
    /// falls back to the process `PATH` (the only process-environment read in
    /// this crate). `from_env_lookup` populates it from `PATH`.
    pub process_path: Option<OsString>,
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
    /// Semicolon separated mount points to include in disk usage. Empty means
    /// "detect every physical filesystem", matching Go's `include_mountpoints`.
    pub include_mountpoints: String,
    /// Comma separated interface patterns to include. Supports Go `filepath.Match`
    /// style wildcards (`*`, `?`, `[...]`).
    pub include_nics: String,
    /// Comma separated interface patterns to exclude.
    pub exclude_nics: String,
    /// Day of month (1-31) on which monthly traffic accounting resets. `0`
    /// disables the monthly database and reports lifetime counters.
    pub month_rotate: i64,
    /// Enable detailed GPU probing (`--enable-gpu`). Basic GPU name detection
    /// used by the basic-info payload is independent of this switch.
    pub enable_gpu: bool,
    /// Window between the two network counter samples used to compute the
    /// current transfer rate. The Go agent uses one second.
    pub network_sample_interval: Duration,
    /// Location of the persistent monthly traffic database
    /// (`net_static.json` in the Go agent's working directory).
    pub net_static_path: PathBuf,
    /// Path of `nvidia-smi`; when missing, `nvidia-smi` is looked up on
    /// [`MonitorOptions::gpu_tool_path`].
    pub nvidia_smi_path: PathBuf,
    /// Path of `rocm-smi`; when missing, it is looked up on
    /// [`MonitorOptions::gpu_tool_path`].
    pub rocm_smi_path: PathBuf,
    /// `PATH` used to locate GPU tools (`nvidia-smi`, `rocm-smi`, `lspci`).
    /// `None` inherits the process `PATH`.
    pub gpu_tool_path: Option<OsString>,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from(DEFAULT_PROC_ROOT),
            sys_root: PathBuf::from(DEFAULT_SYS_ROOT),
            root: PathBuf::from("/"),
            process_path: None,
            memory_include_cache: false,
            memory_report_raw_used: false,
            strict_proc: false,
            cpu_sample_interval: Duration::from_secs(1),
            include_mountpoints: String::new(),
            include_nics: String::new(),
            exclude_nics: String::new(),
            month_rotate: 0,
            enable_gpu: false,
            network_sample_interval: Duration::from_secs(1),
            net_static_path: PathBuf::from("./net_static.json"),
            nvidia_smi_path: PathBuf::from("/usr/bin/nvidia-smi"),
            rocm_smi_path: PathBuf::from("/opt/rocm/bin/rocm-smi"),
            gpu_tool_path: None,
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

    /// Build options from the full set of values resolved by the configuration
    /// layer.
    ///
    /// This is the A05-friendly constructor: it keeps [`MonitorOptions::from_config`]
    /// behaviour for `host_proc` and memory modes, and additionally wires the
    /// disk/network/GPU switches that mirror the Go agent's flags.
    #[allow(clippy::too_many_arguments)]
    pub fn from_monitor_config(
        host_proc: &str,
        memory_include_cache: bool,
        memory_report_raw_used: bool,
        include_mountpoints: &str,
        include_nics: &str,
        exclude_nics: &str,
        month_rotate: i64,
        enable_gpu: bool,
    ) -> Self {
        let mut options =
            Self::from_config(host_proc, memory_include_cache, memory_report_raw_used);
        options.include_mountpoints = include_mountpoints.to_owned();
        options.include_nics = include_nics.to_owned();
        options.exclude_nics = exclude_nics.to_owned();
        options.month_rotate = month_rotate;
        options.enable_gpu = enable_gpu;
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
            // A resolved HOST_PROC is authoritative, like `from_config`.
            options.strict_proc = true;
        }
        if let Some(value) = lookup(HOST_SYS_ENV).filter(|value| !value.trim().is_empty()) {
            options.sys_root = PathBuf::from(value.trim());
        }
        if let Some(value) = lookup(PATH_ENV).filter(|value| !value.is_empty()) {
            options.process_path = Some(OsString::from(value));
        }
        options
    }

    /// Override the proc filesystem root.
    ///
    /// Like `HOST_PROC`, an explicit custom proc root enables
    /// [`MonitorOptions::strict_proc`] so an unreadable mount cannot silently
    /// substitute local metrics. Call [`MonitorOptions::with_strict_proc`] with
    /// `false` to opt back into the fallback.
    pub fn with_proc_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.proc_root = root.into();
        self.strict_proc = true;
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

    /// Override the `PATH` used to locate external commands such as
    /// `pveversion`.
    pub fn with_process_path(mut self, path: impl Into<OsString>) -> Self {
        self.process_path = Some(path.into());
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

    /// Override the disk/network filters and the monthly reset day.
    pub fn with_filters(
        mut self,
        include_mountpoints: impl Into<String>,
        include_nics: impl Into<String>,
        exclude_nics: impl Into<String>,
        month_rotate: i64,
    ) -> Self {
        self.include_mountpoints = include_mountpoints.into();
        self.include_nics = include_nics.into();
        self.exclude_nics = exclude_nics.into();
        self.month_rotate = month_rotate;
        self
    }

    /// Enable or disable detailed GPU probing.
    pub fn with_gpu_enabled(mut self, enabled: bool) -> Self {
        self.enable_gpu = enabled;
        self
    }

    /// Override the network sampling window.
    pub fn with_network_sample_interval(mut self, interval: Duration) -> Self {
        self.network_sample_interval = interval;
        self
    }

    /// Override the monthly traffic database path.
    pub fn with_net_static_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.net_static_path = path.into();
        self
    }

    /// Override the GPU tool paths and the `PATH` used to look them up.
    pub fn with_gpu_tools(
        mut self,
        nvidia_smi: impl Into<PathBuf>,
        rocm_smi: impl Into<PathBuf>,
        gpu_tool_path: Option<OsString>,
    ) -> Self {
        self.nvidia_smi_path = nvidia_smi.into();
        self.rocm_smi_path = rocm_smi.into();
        self.gpu_tool_path = gpu_tool_path;
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
