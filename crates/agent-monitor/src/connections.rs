//! TCP/UDP socket counting.
//!
//! Mirrors `monitoring/unit/net.go`'s Linux fast path: count data rows in
//! `<proc_root>/net/tcp`, `tcp6`, `udp` and `udp6`. A missing file is skipped
//! (a kernel without IPv6 has no `tcp6`); if none of a family's files can be
//! read the collection fails and the report falls back to zero.

use std::path::Path;

use crate::options::MonitorOptions;
use crate::types::ConnectionsCount;

/// Count TCP and UDP sockets.
pub fn count_proc_net(root: &Path) -> Result<ConnectionsCount, String> {
    let tcp = count_proc_net_files(root, &["tcp", "tcp6"])?;
    let udp = count_proc_net_files(root, &["udp", "udp6"])?;
    Ok(ConnectionsCount {
        tcp: i64::try_from(tcp).unwrap_or(i64::MAX),
        udp: i64::try_from(udp).unwrap_or(i64::MAX),
    })
}

/// Count rows in a group of `/proc/net` files.
pub fn count_proc_net_files(root: &Path, names: &[&str]) -> Result<usize, String> {
    let mut total = 0usize;
    let mut read_any = false;

    for name in names {
        let path = root.join("net").join(name);
        match count_proc_net_file(&path) {
            Ok(count) => {
                total = total.saturating_add(count);
                read_any = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
        }
    }

    if !read_any {
        return Err(format!(
            "no proc net files found under {}",
            root.join("net").display()
        ));
    }
    Ok(total)
}

fn count_proc_net_file(path: &Path) -> std::io::Result<usize> {
    let content = std::fs::read_to_string(path)?;
    Ok(content
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .count())
}

/// Collect connection counts using the configured proc root.
pub(crate) fn collect(options: &MonitorOptions) -> Result<ConnectionsCount, String> {
    #[cfg(target_os = "linux")]
    {
        count_proc_net(&options.proc_root)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        Err("connection counting is only implemented on Linux".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, name: &str, body: &str) {
        let directory = root.join("net");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(name), body).unwrap();
    }

    #[test]
    fn counts_tcp_tcp6_udp_udp6_rows_without_headers() {
        let directory = tempfile::TempDir::new().unwrap();
        write(
            directory.path(),
            "tcp",
            "  sl  local_address rem_address   st\n   0: 0100007F:1F90 00000000:0000 0A 0 0\n   1: 0100007F:1F91 00000000:0000 0A 0 0\n",
        );
        write(
            directory.path(),
            "tcp6",
            "  sl  local_address rem_address   st\n   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 0 0\n",
        );
        write(
            directory.path(),
            "udp",
            "  sl  local_address rem_address   st\n   0: 00000000:0035 00000000:0000 07 0 0\n",
        );
        write(
            directory.path(),
            "udp6",
            "  sl  local_address rem_address   st\n   0: 00000000000000000000000000000000:0035 00000000000000000000000000000000:0000 07 0 0\n   1: 00000000000000000000000000000000:0036 00000000000000000000000000000000:0000 07 0 0\n",
        );

        let counts = count_proc_net(directory.path()).unwrap();
        assert_eq!(counts.tcp, 3);
        assert_eq!(counts.udp, 3);
    }

    #[test]
    fn missing_family_files_are_skipped_but_all_missing_is_an_error() {
        let directory = tempfile::TempDir::new().unwrap();
        write(
            directory.path(),
            "tcp",
            "header\n   0: 0100007F:1F90 00000000:0000 0A 0 0\n",
        );
        write(
            directory.path(),
            "udp",
            "header\n   0: 00000000:0035 00000000:0000 07 0 0\n",
        );
        let counts = count_proc_net(directory.path()).unwrap();
        assert_eq!(counts.tcp, 1);
        assert_eq!(counts.udp, 1);

        let empty = tempfile::TempDir::new().unwrap();
        let error = count_proc_net(empty.path()).unwrap_err();
        assert!(error.contains("no proc net files found"), "{error}");
    }

    #[test]
    fn blank_rows_are_ignored() {
        let directory = tempfile::TempDir::new().unwrap();
        write(directory.path(), "tcp", "header\n\n   0: row\n   \n");
        assert_eq!(count_proc_net_files(directory.path(), &["tcp"]).unwrap(), 1);
    }
}
