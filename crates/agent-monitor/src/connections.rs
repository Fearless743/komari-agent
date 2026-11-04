//! TCP/UDP socket counting.
//!
//! Linux mirrors `monitoring/unit/net.go`'s fast path: count data rows in
//! `<proc_root>/net/tcp`, `tcp6`, `udp` and `udp6`. A missing file is skipped
//! (a kernel without IPv6 has no `tcp6`); if none of a family's files can be
//! read the collection fails and the report falls back to zero.
//!
//! macOS/FreeBSD run `lsof -a -n -P -i tcp|udp`, matching gopsutil's
//! `ConnectionsWithContext`. Windows uses `GetExtendedTcpTable` /
//! `GetExtendedUdpTable` (`TCP_TABLE_OWNER_PID_ALL` / `UDP_TABLE_OWNER_PID`)
//! for IPv4 and IPv6, matching gopsutil's `net_windows.go`.

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

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    {
        let _ = options;
        count_lsof()
    }

    #[cfg(windows)]
    {
        let _ = options;
        Ok(windows_impl::count())
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        windows
    )))]
    {
        let _ = options;
        Err("connection counting is not implemented on this platform".to_owned())
    }
}

/// Count `lsof` socket rows.
///
/// `lsof -i` output starts with a `COMMAND ...` header. gopsutil's
/// `parseNetLine` accepts rows with at least eight fields whose family column
/// is `IPv4`/`IPv6` and whose type column is `TCP`/`UDP`; malformed rows are
/// skipped. This helper mirrors that filter.
#[cfg(any(target_os = "macos", target_os = "freebsd", test))]
pub fn count_lsof_rows(output: &str) -> i64 {
    let count = output
        .lines()
        .skip(1)
        .filter(|line| is_lsof_socket_row(line))
        .count();
    i64::try_from(count).unwrap_or(i64::MAX)
}

#[cfg(any(target_os = "macos", target_os = "freebsd", test))]
fn is_lsof_socket_row(line: &str) -> bool {
    let fields: Vec<&str> = line.split_whitespace().collect();
    fields.len() >= 8 && matches!(fields[4], "IPv4" | "IPv6") && matches!(fields[7], "TCP" | "UDP")
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn count_lsof() -> Result<ConnectionsCount, String> {
    let tcp = count_lsof_kind("tcp")?;
    let udp = count_lsof_kind("udp")?;
    Ok(ConnectionsCount {
        tcp: i64::try_from(tcp).unwrap_or(i64::MAX),
        udp: i64::try_from(udp).unwrap_or(i64::MAX),
    })
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn count_lsof_kind(kind: &str) -> Result<usize, String> {
    use std::process::Command;

    // Same invocation as gopsutil's CallLsofWithContext for pid 0.
    let output = Command::new("lsof")
        .args(["-a", "-n", "-P", "-i", kind])
        .output()
        .map_err(|error| format!("failed to run lsof: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() && stdout.trim().is_empty() {
        // lsof exits 1 when no socket matches; gopsutil treats that as empty.
        return Ok(0);
    }

    let count = usize::try_from(count_lsof_rows(&stdout)).unwrap_or(usize::MAX);
    Ok(count)
}

/// Read the entry count from a Windows `MIB_*TABLE_OWNER_PID` buffer.
///
/// Both tables start with a `u32 dwNumEntries` followed by variable-length
/// rows, so only the header is needed to count connections. Kept platform
/// independent for the Linux test suite.
#[cfg(any(windows, test))]
pub fn table_entry_count(buffer: &[u8]) -> u32 {
    match buffer.get(..4) {
        Some(bytes) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        None => 0,
    }
}

#[cfg(windows)]
mod windows_impl {
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    use super::{ConnectionsCount, table_entry_count};

    /// Query both address families and both protocols.
    ///
    /// gopsutil's `getProcInet` ignores per-family failures and keeps the
    /// entries it did collect, so a failed family contributes zero instead of
    /// failing the whole report.
    pub(super) fn count() -> ConnectionsCount {
        let mut tcp = 0u32;
        let mut udp = 0u32;

        for family in [AF_INET, AF_INET6] {
            match query_tcp(u32::from(family.0)) {
                Ok(count) => tcp = tcp.saturating_add(count),
                Err(error) => tracing::debug!(%error, "GetExtendedTcpTable failed"),
            }
            match query_udp(u32::from(family.0)) {
                Ok(count) => udp = udp.saturating_add(count),
                Err(error) => tracing::debug!(%error, "GetExtendedUdpTable failed"),
            }
        }

        ConnectionsCount {
            tcp: i64::from(tcp),
            udp: i64::from(udp),
        }
    }

    fn query_tcp(family: u32) -> Result<u32, String> {
        let mut size = 0u32;
        // SAFETY: a null buffer asks for the required size; `size` is writable.
        let status = unsafe {
            GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        if status == 0 {
            return Ok(0);
        }
        if status != ERROR_INSUFFICIENT_BUFFER.0 {
            return Err(format!("GetExtendedTcpTable failed with error {status}"));
        }

        let mut buffer = vec![0u8; usize::try_from(size).unwrap_or(0)];
        if buffer.is_empty() {
            return Ok(0);
        }
        // SAFETY: `buffer` has `size` bytes and stays alive for the call.
        let status = unsafe {
            GetExtendedTcpTable(
                Some(buffer.as_mut_ptr().cast()),
                &mut size,
                false,
                family,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if status != 0 {
            return Err(format!("GetExtendedTcpTable failed with error {status}"));
        }
        Ok(table_entry_count(&buffer))
    }

    fn query_udp(family: u32) -> Result<u32, String> {
        let mut size = 0u32;
        // SAFETY: a null buffer asks for the required size; `size` is writable.
        let status =
            unsafe { GetExtendedUdpTable(None, &mut size, false, family, UDP_TABLE_OWNER_PID, 0) };
        if status == 0 {
            return Ok(0);
        }
        if status != ERROR_INSUFFICIENT_BUFFER.0 {
            return Err(format!("GetExtendedUdpTable failed with error {status}"));
        }

        let mut buffer = vec![0u8; usize::try_from(size).unwrap_or(0)];
        if buffer.is_empty() {
            return Ok(0);
        }
        // SAFETY: `buffer` has `size` bytes and stays alive for the call.
        let status = unsafe {
            GetExtendedUdpTable(
                Some(buffer.as_mut_ptr().cast()),
                &mut size,
                false,
                family,
                UDP_TABLE_OWNER_PID,
                0,
            )
        };
        if status != 0 {
            return Err(format!("GetExtendedUdpTable failed with error {status}"));
        }
        Ok(table_entry_count(&buffer))
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

    #[test]
    fn lsof_rows_skip_header_and_non_socket_rows() {
        let output = "\
COMMAND     PID USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME
launchd       1 root   28u  IPv4 0x1234567890abcdef      0t0  TCP *:22 (LISTEN)
launchd       1 root   29u  IPv6 0x1234567890abcdee      0t0  TCP *:22 (LISTEN)
mDNSRespo   123 _mdns  5u  IPv4 0x1234567890abcde0      0t0  UDP *:5353
bash        456 user    3r   REG                1,5     1234  123 /etc/hosts
";
        assert_eq!(count_lsof_rows(output), 3);
        assert_eq!(
            count_lsof_rows("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n"),
            0
        );
        assert_eq!(count_lsof_rows(""), 0);
    }

    #[test]
    fn windows_table_entry_count_reads_the_header() {
        let mut buffer = vec![0u8; 8];
        buffer[..4].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(table_entry_count(&buffer), 7);
        assert_eq!(table_entry_count(&[]), 0);
        assert_eq!(table_entry_count(&[1, 2, 3]), 0);
    }
}
