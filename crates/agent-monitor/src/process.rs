//! Process counting.
//!
//! Linux counts numeric entries below the injected proc root, matching
//! `monitoring/unit/process_linux.go` (and therefore honours `HOST_PROC`).
//! macOS/FreeBSD run `ps` like `process_{darwin,freebsd}.go`; Windows uses
//! `K32EnumProcesses` like `process_windows.go`. Remaining platforms fall back
//! to `sysinfo`.

use crate::options::MonitorOptions;

/// Count running processes.
pub(crate) fn collect(options: &MonitorOptions) -> i64 {
    #[cfg(target_os = "linux")]
    {
        count_proc(&options.proc_root)
    }

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    {
        let _ = options;
        count_ps()
    }

    #[cfg(windows)]
    {
        let _ = options;
        windows_impl::count()
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        windows
    )))]
    {
        let _ = options;
        count_sysinfo()
    }
}

/// Count numeric directories in a proc filesystem root.
pub fn count_proc(root: &std::path::Path) -> i64 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };

    let count = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.parse::<i64>().is_ok())
        })
        .count();
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Count process rows in `ps` output.
///
/// The first line is the header and blank lines are ignored. Go's
/// `process_{darwin,freebsd}.go` subtracts one from `strings.Split(output,
/// "\n")`, which counts the header as a process (D-025); this helper counts
/// the actual rows instead.
#[cfg(any(target_os = "macos", target_os = "freebsd", test))]
pub fn count_ps_rows(output: &str) -> i64 {
    let count = output
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .count();
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Convert the byte count reported by `EnumProcesses` into a PID count.
///
/// Kept platform independent so the arithmetic is covered by the Linux test
/// suite; the Windows caller owns the API buffer handling.
#[cfg(any(windows, test))]
pub fn count_pids_from_bytes(bytes_returned: u32) -> i64 {
    i64::from(bytes_returned / 4)
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn count_ps() -> i64 {
    #[cfg(target_os = "macos")]
    const ARGS: [&str; 1] = ["-A"];
    #[cfg(target_os = "freebsd")]
    const ARGS: [&str; 1] = ["-ax"];

    match std::process::Command::new("ps").args(ARGS).output() {
        Ok(output) if output.status.success() => {
            count_ps_rows(&String::from_utf8_lossy(&output.stdout))
        }
        Ok(output) => {
            tracing::debug!(
                status = %output.status,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "ps failed; reporting zero processes"
            );
            0
        }
        Err(error) => {
            tracing::debug!(%error, "failed to run ps; reporting zero processes");
            0
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    windows
)))]
fn count_sysinfo() -> i64 {
    use sysinfo::System;

    let Ok(mut system) = System::new_all() else {
        return 0;
    };
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    i64::try_from(system.processes().len()).unwrap_or(i64::MAX)
}

#[cfg(windows)]
mod windows_impl {
    use windows::Win32::System::ProcessStatus::K32EnumProcesses;

    use super::count_pids_from_bytes;

    /// Initial PID buffer, matching the fixed 1024-entry buffer used by Go's
    /// `process_windows.go`. Unlike Go, the buffer is grown when
    /// `EnumProcesses` reports it was too small (D-025).
    const INITIAL_PIDS: usize = 1024;
    const MAX_PIDS: usize = 1 << 20;

    pub(super) fn count() -> i64 {
        let mut capacity = INITIAL_PIDS;

        loop {
            let mut pids = vec![0u32; capacity];
            let mut needed = 0u32;
            // SAFETY: `pids` is valid for `capacity * 4` bytes and `needed`
            // points to a writable u32.
            let ok = unsafe {
                K32EnumProcesses(
                    pids.as_mut_ptr(),
                    u32::try_from(capacity * std::mem::size_of::<u32>()).unwrap_or(u32::MAX),
                    &mut needed,
                )
            };
            if !ok.as_bool() {
                return 0;
            }

            let count = count_pids_from_bytes(needed);
            // `needed` equals the buffer size when it was too small; retry with
            // a larger buffer so machines with more than 1024 processes are
            // counted accurately.
            if usize::try_from(count).unwrap_or(usize::MAX) < capacity {
                return count;
            }
            if capacity >= MAX_PIDS {
                return count;
            }
            capacity = capacity.saturating_mul(2).min(MAX_PIDS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_numeric_proc_entries() {
        let directory = tempfile::TempDir::new().unwrap();
        for name in ["1", "42", "999999", "self", "sys", "net", "uptime"] {
            std::fs::create_dir_all(directory.path().join(name)).unwrap();
        }
        std::fs::write(directory.path().join("1234"), "not a directory").unwrap();

        assert_eq!(count_proc(directory.path()), 4);
    }

    #[test]
    fn missing_proc_root_reports_zero() {
        assert_eq!(
            count_proc(std::path::Path::new("/definitely/not/a/proc/root")),
            0
        );
    }

    #[test]
    fn ps_rows_skip_header_and_blank_lines() {
        let output = "  PID TTY          TIME CMD\n    1 ??         0:01.23 launchd\n  42 ttys000    0:00.01 -zsh\n  77 ttys001    0:00.00 ps\n";
        assert_eq!(count_ps_rows(output), 3);
        assert_eq!(count_ps_rows("  PID TTY          TIME CMD\n"), 0);
        assert_eq!(count_ps_rows(""), 0);
    }

    #[test]
    fn enum_processes_byte_count_is_divided_by_four() {
        assert_eq!(count_pids_from_bytes(0), 0);
        assert_eq!(count_pids_from_bytes(4), 1);
        assert_eq!(count_pids_from_bytes(1024 * 4), 1024);
        // A partial trailing record is truncated, like Go's integer division.
        assert_eq!(count_pids_from_bytes(10), 2);
    }
}
