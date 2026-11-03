//! Memory and swap collection.
//!
//! The accounting modes mirror the Go agent:
//!
//! | Go flag / platform        | Mode           | Used calculation |
//! |---------------------------|----------------|------------------|
//! | `memory_include_cache`    | `includeCache` | `total - free` |
//! | `memory_report_raw_used`  | `htoplike`     | `total - (free + cached + sreclaimable + buffers) + shmem` |
//! | Linux default             | `htoplike`     | same as above |
//! | non-Linux default         | `gopsutil`     | `total - available` |

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::options::MonitorOptions;
use crate::platform;
use crate::procfs::ProcMemInfo;
use crate::types::{MemoryInfo, MemoryMode, SwapInfo};

#[cfg(target_os = "linux")]
use crate::procfs::read_meminfo;

/// Select the accounting mode using Go's precedence:
/// `include_cache` first, then `report_raw_used`, then the platform default.
pub(crate) fn mode(options: &MonitorOptions) -> MemoryMode {
    if options.memory_include_cache {
        MemoryMode::IncludeCache
    } else if options.memory_report_raw_used {
        MemoryMode::HtopLike
    } else {
        #[cfg(target_os = "linux")]
        const DEFAULT_MODE: MemoryMode = MemoryMode::HtopLike;
        #[cfg(not(target_os = "linux"))]
        const DEFAULT_MODE: MemoryMode = MemoryMode::Gopsutil;
        DEFAULT_MODE
    }
}

/// Collect RAM information.
pub(crate) fn collect(options: &MonitorOptions) -> MemoryInfo {
    #[cfg(target_os = "linux")]
    {
        match read_meminfo(&options.proc_root) {
            Ok(info) if info.mem_total > 0 => return from_meminfo(&info, mode(options)),
            Ok(_) => {}
            Err(error) => {
                platform::warn_proc_failure(options, "meminfo", &error);
            }
        }
    }

    if options.strict_proc {
        return empty_memory(mode(options));
    }

    platform::fallback::memory(mode(options))
}

fn empty_memory(mode: MemoryMode) -> MemoryInfo {
    MemoryInfo {
        mode,
        ..Default::default()
    }
}

/// Build a [`MemoryInfo`] from raw `/proc/meminfo` counters.
pub(crate) fn from_meminfo(info: &ProcMemInfo, mode: MemoryMode) -> MemoryInfo {
    let total = info.mem_total;
    let used = match mode {
        MemoryMode::IncludeCache => total.saturating_sub(info.mem_free),
        MemoryMode::HtopLike => info.htop_used(),
        MemoryMode::Gopsutil => total.saturating_sub(info.available()),
    };

    MemoryInfo {
        total,
        used,
        available: info.available(),
        free: info.mem_free,
        mode,
    }
}

/// Collect swap information.
pub(crate) fn collect_swap(options: &MonitorOptions) -> SwapInfo {
    #[cfg(not(target_os = "linux"))]
    let _ = options;

    #[cfg(target_os = "linux")]
    {
        match read_meminfo(&options.proc_root) {
            Ok(info) => return swap_from_meminfo(&info),
            Err(error) => {
                platform::warn_proc_failure(options, "meminfo", &error);
            }
        }
    }

    if options.strict_proc {
        return SwapInfo::default();
    }

    platform::fallback::swap()
}

/// Swap used is `total - free - cached`, with the same underflow guard as Go.
pub(crate) fn swap_from_meminfo(info: &ProcMemInfo) -> SwapInfo {
    let total = info.swap_total;
    let deductions = info.swap_free.saturating_add(info.swap_cached);
    let used = if total >= deductions {
        total - deductions
    } else {
        total.saturating_sub(info.swap_free)
    };

    SwapInfo { total, used }
}
