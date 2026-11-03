//! Load average collection.

use crate::options::MonitorOptions;
use crate::platform;
use crate::procfs::read_loadavg;
use crate::types::LoadAverage;

/// Collect the 1/5/15 minute load averages.
///
/// Linux reads `/proc/loadavg`; other platforms, and failed proc reads, fall
/// back to `sysinfo`. Unsupported platforms (notably Windows) report zeros.
pub(crate) fn collect(options: &MonitorOptions) -> LoadAverage {
    #[cfg(target_os = "linux")]
    {
        if let Ok(load) = read_loadavg(&options.proc_root) {
            return load;
        }
    }

    platform::fallback::load()
}
