//! CPU collection.

use std::collections::HashSet;

#[cfg(target_os = "linux")]
use std::fs;

use parking_lot::Mutex;

use crate::options::MonitorOptions;
use crate::platform;
use crate::procfs::ProcStat;

#[cfg(target_os = "linux")]
use crate::procfs::{read_proc_stat, usage_between};
use crate::types::CpuInfo;

/// Collect CPU information.
///
/// On Linux the usage is computed from two `/proc/stat` samples. The latest
/// sample is cached in `last`, so only the first call has to sleep; subsequent
/// calls measure the interval since the previous call. The Go agent samples for
/// one second on every call.
pub(crate) fn collect(options: &MonitorOptions, last: &Mutex<Option<ProcStat>>) -> CpuInfo {
    #[cfg(target_os = "linux")]
    {
        let mut last = last.lock();
        if let Some(info) = sample_proc(options, &mut last) {
            return info;
        }
    }

    #[cfg(not(target_os = "linux"))]
    let _ = last;

    platform::fallback::cpu_info(options.cpu_sample_interval)
}

#[cfg(target_os = "linux")]
fn sample_proc(options: &MonitorOptions, last: &mut Option<ProcStat>) -> Option<CpuInfo> {
    let previous = match last.take() {
        Some(previous) => previous,
        None => {
            let first = read_proc_stat(&options.proc_root).ok()?;
            std::thread::sleep(options.cpu_sample_interval);
            first
        }
    };

    let current = read_proc_stat(&options.proc_root).ok()?;
    let usage = usage_between(&previous, &current);

    let (mut name, physical_cores) = read_cpu_info(&options.proc_root)
        .map(|content| parse_cpuinfo(&content))
        .unwrap_or((None, 0));

    if name.is_none() {
        name = platform::fallback::cpu_brand();
    }

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

    Some(CpuInfo {
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

/// Extract the CPU model name and physical core count from `/proc/cpuinfo`.
///
/// x86 exposes `physical id` + `core id` pairs; ARM boards often omit them, in
/// which case the physical core count is reported as `0` (unknown), matching
/// the Go agent's backwards-compatible default.
pub(crate) fn parse_cpuinfo(content: &str) -> (Option<String>, u32) {
    let mut model_name = None;
    let mut hardware = None;
    let mut processor = None;
    let mut model = None;
    let mut physical_cores = HashSet::new();
    let mut physical_id: Option<&str> = None;
    let mut core_id: Option<&str> = None;

    // The trailing empty line flushes the final block.
    for line in content.lines().chain(std::iter::once("")) {
        let line = line.trim();
        if line.is_empty() {
            let physical = physical_id.take();
            let core = core_id.take();
            if let (Some(physical), Some(core)) = (physical, core) {
                physical_cores.insert((physical.to_owned(), core.to_owned()));
            }
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
            "model name" if model_name.is_none() => model_name = Some(value.to_owned()),
            "Hardware" if hardware.is_none() => hardware = Some(value.to_owned()),
            "Processor" if processor.is_none() => processor = Some(value.to_owned()),
            "Model" if model.is_none() => model = Some(value.to_owned()),
            "physical id" => physical_id = Some(value),
            "core id" => core_id = Some(value),
            _ => {}
        }
    }

    let name = model_name.or(hardware).or(processor).or(model);
    let physical_cores = u32::try_from(physical_cores.len()).unwrap_or(u32::MAX);

    (name, physical_cores)
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
    fn arm_boards_report_unknown_physical_cores() {
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
        assert_eq!(physical, 0);
    }

    #[test]
    fn prefers_model_name_and_ignores_empty_values() {
        let content = "\
Hardware\t: Example Board
Processor\t: Example Processor
Model\t\t: Example Model
model name\t: \t
";
        let (name, physical) = parse_cpuinfo(content);
        assert_eq!(name.as_deref(), Some("Example Board"));
        assert_eq!(physical, 0);
    }

    #[test]
    fn empty_input_yields_unknown() {
        let (name, physical) = parse_cpuinfo("");
        assert_eq!(name, None);
        assert_eq!(physical, 0);
    }
}
