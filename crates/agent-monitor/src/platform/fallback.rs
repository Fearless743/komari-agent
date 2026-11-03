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
