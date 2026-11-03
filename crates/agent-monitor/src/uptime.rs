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
    #[cfg(target_os = "linux")]
    {
        if let Ok(uptime) = read_uptime(&options.proc_root) {
            return uptime;
        }
    }

    platform::fallback::uptime()
}
