//! `sysinfo`-based fallback used on non-Linux platforms and whenever the
//! configured proc files are unreadable.

use std::time::Duration;

use sysinfo::System;

use crate::types::CpuInfo;

/// CPU information from `sysinfo`.
///
/// `sysinfo` needs two refreshes separated by at least
/// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] to compute a usage value, so this
/// blocks once per call. Linux normally uses `/proc/stat` instead.
pub(crate) fn cpu_info(sample_interval: Duration) -> CpuInfo {
    let mut info = CpuInfo::unknown(crate::os::arch());

    let Ok(mut system) = System::new_all() else {
        return info;
    };
    system.refresh_cpu_all();
    std::thread::sleep(sample_interval.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
    system.refresh_cpu_all();

    let cpus = system.cpus();
    if !cpus.is_empty() {
        info.cores = u32::try_from(cpus.len()).unwrap_or(u32::MAX);
        info.per_core_usage = cpus.iter().map(|cpu| f64::from(cpu.usage())).collect();
        info.usage = info.per_core_usage.iter().sum::<f64>() / info.per_core_usage.len() as f64;
        info.name = cpu_brand().unwrap_or_else(|| "Unknown".to_owned());
    }

    info.physical_cores = u32::try_from(System::physical_core_count().unwrap_or(0)).unwrap_or(0);
    info
}

/// Best-effort CPU brand string from `sysinfo`.
pub(crate) fn cpu_brand() -> Option<String> {
    let system = System::new_all().ok()?;
    system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_owned())
        .filter(|brand| !brand.is_empty())
}

/// RAM information from `sysinfo`.
///
/// `includeCache` maps to `total - free`; the other modes map to
/// `total - available`, which is what gopsutil reports on non-Linux systems.
pub(crate) fn memory(mode: crate::types::MemoryMode) -> crate::types::MemoryInfo {
    let Ok(system) = System::new_all() else {
        return crate::types::MemoryInfo {
            mode,
            ..Default::default()
        };
    };

    let total = system.total_memory();
    let free = system.free_memory();
    let available = system.available_memory().min(total);
    let used = match mode {
        crate::types::MemoryMode::IncludeCache => total.saturating_sub(free),
        _ => total.saturating_sub(available),
    };

    crate::types::MemoryInfo {
        total,
        used,
        available,
        free,
        mode,
    }
}

/// Swap information from `sysinfo`.
pub(crate) fn swap() -> crate::types::SwapInfo {
    let Ok(system) = System::new_all() else {
        return crate::types::SwapInfo::default();
    };

    crate::types::SwapInfo {
        total: system.total_swap(),
        used: system.used_swap(),
    }
}

/// Load averages from `sysinfo`.
///
/// Windows does not expose load averages; `sysinfo` returns an error there and
/// the collector reports zeros, matching the Go agent's degradation.
pub(crate) fn load() -> crate::types::LoadAverage {
    System::load_average()
        .map(|load| crate::types::LoadAverage {
            load1: load.one,
            load5: load.five,
            load15: load.fifteen,
        })
        .unwrap_or_default()
}

/// Uptime in seconds from `sysinfo`.
pub(crate) fn uptime() -> u64 {
    System::uptime().unwrap_or(0)
}

/// Best-effort OS name for non-Linux platforms.
#[cfg(target_os = "linux")]
pub(crate) fn os_name() -> String {
    "Linux".to_owned()
}

/// Best-effort OS name for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub(crate) fn os_name() -> String {
    System::long_os_version()
        .or_else(|| System::name().ok())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| std::env::consts::OS.to_owned())
}

/// Kernel release from `sysinfo`.
pub(crate) fn kernel_version() -> String {
    System::kernel_version()
        .ok()
        .filter(|version| !version.trim().is_empty())
        .unwrap_or_else(|| "Unknown".to_owned())
}
