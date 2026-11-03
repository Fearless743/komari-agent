//! Process counting.
//!
//! Linux counts numeric entries below the injected proc root, matching
//! `monitoring/unit/process_linux.go` (and therefore honours `HOST_PROC`).
//! Other platforms use `sysinfo`.

use crate::options::MonitorOptions;

/// Count running processes.
pub(crate) fn collect(options: &MonitorOptions) -> i64 {
    #[cfg(target_os = "linux")]
    {
        count_proc(&options.proc_root)
    }

    #[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
fn count_sysinfo() -> i64 {
    use sysinfo::System;

    let Ok(mut system) = System::new_all() else {
        return 0;
    };
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    i64::try_from(system.processes().len()).unwrap_or(i64::MAX)
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
}
