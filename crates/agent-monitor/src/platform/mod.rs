//! Platform fallbacks.
//!
//! Linux is the full implementation. Other platforms use `sysinfo` for CPU,
//! memory, swap, load, uptime, OS and network counters. The fallback is also
//! compiled on Linux and used when proc reads for those metrics fail (unless
//! `strict_proc` is set). Exceptions: Linux disk does not fall back to
//! `sysinfo` when the mount table is unreadable, and process count /
//! connections have their own platform paths. Keeping the fallback compiled on
//! Linux means its API usage is checked by the normal test suite.

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
use crate::error::MonitorError;
#[cfg(target_os = "linux")]
use crate::options::MonitorOptions;

pub(crate) mod fallback;

/// Emit a single process-wide warning when proc metrics cannot be read.
///
/// In strict mode the collector returns empty values rather than substituting
/// local system data, which keeps a broken `HOST_PROC` mount visible.
#[cfg(target_os = "linux")]
pub(crate) fn warn_proc_failure(options: &MonitorOptions, metric: &str, error: &MonitorError) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }

    if options.strict_proc {
        tracing::warn!(
            proc_root = %options.proc_root.display(),
            metric,
            error = %error,
            "failed to read proc metrics; strict proc mode returns empty values"
        );
    } else {
        tracing::warn!(
            proc_root = %options.proc_root.display(),
            metric,
            error = %error,
            "failed to read proc metrics; falling back to local system data"
        );
    }
}
