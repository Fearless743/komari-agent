//! CPU collection.

use std::collections::{HashMap, HashSet};

#[cfg(target_os = "linux")]
use std::fs;

use parking_lot::Mutex;

use crate::options::MonitorOptions;
use crate::platform;
use crate::procfs::ProcStat;

#[cfg(target_os = "linux")]
use crate::procfs::{read_proc_stat, usage_between};
use crate::types::CpuInfo;

/// Collect CPU information using the cached sample (non-blocking after the
/// first call).
pub(crate) fn collect(options: &MonitorOptions, last: &Mutex<Option<ProcStat>>) -> CpuInfo {
    collect_with(options, last, None)
}

/// Force a fresh two-sample measurement over `window`, ignoring the cache.
pub(crate) fn sample(
    options: &MonitorOptions,
    last: &Mutex<Option<ProcStat>>,
    window: std::time::Duration,
) -> CpuInfo {
    collect_with(options, last, Some(window))
}

fn collect_with(
    options: &MonitorOptions,
    last: &Mutex<Option<ProcStat>>,
    forced_window: Option<std::time::Duration>,
) -> CpuInfo {
    #[cfg(target_os = "linux")]
    {
        let mut last = last.lock();
        match sample_proc(options, &mut last, forced_window) {
            Ok(info) => return info,
            Err(error) => {
                platform::warn_proc_failure(options, "stat", &error);
                if options.strict_proc {
                    return CpuInfo::unknown(crate::os::arch());
                }
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    let _ = last;

    platform::fallback::cpu_info(forced_window.unwrap_or(options.cpu_sample_interval))
}

#[cfg(target_os = "linux")]
fn sample_proc(
    options: &MonitorOptions,
    last: &mut Option<ProcStat>,
    forced_window: Option<std::time::Duration>,
) -> Result<CpuInfo, crate::MonitorError> {
    let previous = match forced_window {
        Some(window) => {
            let first = read_proc_stat(&options.proc_root)?;
            std::thread::sleep(window);
            first
        }
        None => match last.take() {
            Some(previous) => previous,
            None => {
                let first = read_proc_stat(&options.proc_root)?;
                std::thread::sleep(options.cpu_sample_interval);
                first
            }
        },
    };

    let current = read_proc_stat(&options.proc_root)?;
    let usage = usage_between(&previous, &current);

    let (mut name, cpuinfo_physical_cores) = read_cpu_info(&options.proc_root)
        .map(|content| parse_cpuinfo(&content))
        .unwrap_or((None, 0));

    if name.is_none() {
        name = platform::fallback::cpu_brand();
    }

    // gopsutil counts physical cores from sysfs topology first and only falls
    // back to /proc/cpuinfo when sysfs is unavailable.
    let physical_cores = sysfs_physical_cores(&options.sys_path("devices/system/cpu"))
        .unwrap_or(cpuinfo_physical_cores);

    let cores = u32::try_from(current.cores.len())
        .ok()
        .filter(|cores| *cores > 0)
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .and_then(|cores| u32::try_from(cores.get()).ok())
        })
        .unwrap_or(1);

    *last = Some(current);

    Ok(CpuInfo {
        name: name.unwrap_or_else(|| "Unknown".to_owned()),
        arch: crate::os::arch(),
        cores,
        physical_cores,
        usage: usage.total,
        per_core_usage: usage.per_core,
    })
}

#[cfg(target_os = "linux")]
fn read_cpu_info(proc_root: &std::path::Path) -> Option<String> {
    fs::read_to_string(proc_root.join("cpuinfo")).ok()
}

/// Count physical cores from sysfs CPU topology.
///
/// Mirrors gopsutil's `CountsWithContext`: it tries `core_cpus_list` for every
/// `cpuN`, then the deprecated `thread_siblings_list`, and counts unique
/// sibling sets. Returns `None` when the topology files are unavailable.
#[cfg(target_os = "linux")]
fn sysfs_physical_cores(cpu_dir: &std::path::Path) -> Option<u32> {
    let entries = fs::read_dir(cpu_dir).ok()?;
    let cpu_dirs: Vec<_> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(index) = name.strip_prefix("cpu") else {
                return false;
            };
            !index.is_empty() && index.chars().all(|ch| ch.is_ascii_digit())
        })
        .map(|entry| entry.path())
        .collect();

    for file in ["core_cpus_list", "thread_siblings_list"] {
        let mut siblings = HashSet::new();
        for cpu_dir in &cpu_dirs {
            let Ok(content) = fs::read_to_string(cpu_dir.join("topology").join(file)) else {
                continue;
            };
            if let Some(value) = content.lines().next().map(str::trim)
                && !value.is_empty()
            {
                siblings.insert(value.to_owned());
            }
        }
        if !siblings.is_empty() {
            return u32::try_from(siblings.len()).ok();
        }
    }

    None
}

/// Extract the CPU model name and physical core count from `/proc/cpuinfo`.
///
/// Physical core resolution follows gopsutil's fallback order:
///
/// 1. unique `physical id` + `core id` pairs (x86 with or without SMT);
/// 2. sum of `cpu cores` per `physical id` when `core id` is unavailable;
/// 3. unique `processor` entries when the topology fields are absent (common on
///    ARM64/RISC-V/LoongArch).
///
/// The model name priority follows gopsutil: `model name` (or `Model Name`,
/// `cpu`, `uarch`) first, then `Processor`, then `Hardware`, then `Model`.
pub(crate) fn parse_cpuinfo(content: &str) -> (Option<String>, u32) {
    let mut model_name = None;
    let mut processor_name = None;
    let mut hardware = None;
    let mut model = None;
    let mut pairs = HashSet::new();
    let mut cores_by_socket: HashMap<String, u32> = HashMap::new();
    let mut processors = HashSet::new();
    let mut physical_id: Option<&str> = None;
    let mut core_id: Option<&str> = None;
    let mut cpu_cores: Option<u32> = None;

    // The trailing empty line flushes the final block.
    for line in content.lines().chain(std::iter::once("")) {
        let line = line.trim();
        if line.is_empty() {
            flush_cpuinfo_block(
                &mut pairs,
                &mut cores_by_socket,
                physical_id.take(),
                core_id.take(),
                cpu_cores.take(),
            );
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        match key {
            "model name" | "Model Name" | "cpu" | "uarch" if model_name.is_none() => {
                model_name = Some(value.to_owned());
            }
            "Processor" if processor_name.is_none() => processor_name = Some(value.to_owned()),
            "Hardware" if hardware.is_none() => hardware = Some(value.to_owned()),
            "Model" if model.is_none() => model = Some(value.to_owned()),
            "processor" if value.parse::<u32>().is_ok() => {
                processors.insert(value.to_owned());
            }
            "physical id" => physical_id = Some(value),
            "core id" => core_id = Some(value),
            "cpu cores" => cpu_cores = value.parse::<u32>().ok(),
            _ => {}
        }
    }

    let physical_cores = if !pairs.is_empty() {
        u32::try_from(pairs.len()).unwrap_or(u32::MAX)
    } else if !cores_by_socket.is_empty() {
        cores_by_socket
            .values()
            .copied()
            .fold(0u32, u32::saturating_add)
    } else {
        u32::try_from(processors.len()).unwrap_or(u32::MAX)
    };

    let name = model_name.or(processor_name).or(hardware).or(model);

    (name, physical_cores)
}

fn flush_cpuinfo_block(
    pairs: &mut HashSet<(String, String)>,
    cores_by_socket: &mut HashMap<String, u32>,
    physical_id: Option<&str>,
    core_id: Option<&str>,
    cpu_cores: Option<u32>,
) {
    let Some(physical_id) = physical_id else {
        return;
    };

    if let Some(core_id) = core_id {
        pairs.insert((physical_id.to_owned(), core_id.to_owned()));
    }
    if let Some(cpu_cores) = cpu_cores {
        cores_by_socket
            .entry(physical_id.to_owned())
            .and_modify(|cores| *cores = (*cores).max(cpu_cores))
            .or_insert(cpu_cores);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cpuinfo;

    #[test]
    fn parses_x86_model_and_physical_cores() {
        let content = "\
processor\t: 0
model name\t: Example CPU
physical id\t: 0
core id\t\t: 0

processor\t: 1
model name\t: Example CPU
physical id\t: 0
core id\t\t: 1

processor\t: 2
model name\t: Example CPU
physical id\t: 1
core id\t\t: 0

processor\t: 3
model name\t: Example CPU
physical id\t: 1
core id\t\t: 1

";
        let (name, physical) = parse_cpuinfo(content);
        assert_eq!(name.as_deref(), Some("Example CPU"));
        assert_eq!(physical, 4);
    }

    #[test]
    fn arm_boards_fall_back_to_processor_count() {
        let content = "\
processor\t: 0
model name\t: ARMv8 Processor rev 1 (v8l)
BogoMIPS\t: 108.00
Features\t: fp asimd

processor\t: 1
model name\t: ARMv8 Processor rev 1 (v8l)
BogoMIPS\t: 108.00
Features\t: fp asimd

Hardware\t: Example Board
";
        let (name, physical) = parse_cpuinfo(content);
        assert_eq!(name.as_deref(), Some("ARMv8 Processor rev 1 (v8l)"));
        assert_eq!(physical, 2);
    }

    #[test]
    fn uses_cpu_cores_per_socket_when_core_id_is_missing() {
        let content = "\
processor\t: 0
physical id\t: 0
cpu cores\t: 4

processor\t: 1
physical id\t: 0
cpu cores\t: 4

processor\t: 2
physical id\t: 1
cpu cores\t: 8

processor\t: 3
physical id\t: 1
cpu cores\t: 8
";
        let (_, physical) = parse_cpuinfo(content);
        assert_eq!(physical, 12);
    }

    #[test]
    fn prefers_model_name_then_processor() {
        let content = "\
Hardware\t: Example Board
Processor\t: Example Processor
Model\t\t: Example Model
model name\t: \t
";
        let (name, physical) = parse_cpuinfo(content);
        assert_eq!(name.as_deref(), Some("Example Processor"));
        assert_eq!(physical, 0);
    }

    #[test]
    fn empty_input_yields_unknown() {
        let (name, physical) = parse_cpuinfo("");
        assert_eq!(name, None);
        assert_eq!(physical, 0);
    }
}
