//! Load average collection.

use crate::options::MonitorOptions;
use crate::platform;
use crate::types::LoadAverage;

#[cfg(target_os = "linux")]
use crate::procfs::read_loadavg;

/// Collect the 1/5/15 minute load averages.
///
/// Linux reads `/proc/loadavg`; other platforms, and failed proc reads, fall
/// back to `sysinfo`. Unsupported platforms (notably Windows) report zeros.
pub(crate) fn collect(options: &MonitorOptions) -> LoadAverage {
    #[cfg(not(target_os = "linux"))]
    let _ = options;

    #[cfg(target_os = "linux")]
    {
        match read_loadavg(&options.proc_root) {
            Ok(load) => return load,
            Err(error) => {
                platform::warn_proc_failure(options, "loadavg", &error);
            }
        }
    }

    if options.strict_proc {
        return LoadAverage::default();
    }

    platform::fallback::load()
}
