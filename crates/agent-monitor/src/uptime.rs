//! Uptime collection.

use crate::options::MonitorOptions;
use crate::platform;

#[cfg(target_os = "linux")]
use crate::procfs::read_uptime;

/// Collect uptime in whole seconds.
///
/// Linux reads `/proc/uptime`; other platforms, and failed proc reads, fall
/// back to `sysinfo`.
pub(crate) fn collect(options: &MonitorOptions) -> u64 {
    #[cfg(not(target_os = "linux"))]
    let _ = options;

    #[cfg(target_os = "linux")]
    {
        match read_uptime(&options.proc_root) {
            Ok(uptime) => return uptime,
            Err(error) => {
                platform::warn_proc_failure(options, "uptime", &error);
            }
        }
    }

    if options.strict_proc {
        return 0;
    }

    platform::fallback::uptime()
}
