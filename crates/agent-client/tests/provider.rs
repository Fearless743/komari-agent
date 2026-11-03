//! Direct wiring tests for `MonitorReportProvider`.
//!
//! A temporary proc root feeds a real `agent_monitor::Collector`; the provider
//! must pass A03's disk/network/connections/process/GPU/message through and use
//! the configured fixed CPU window.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_client::{IpOptions, MonitorReportProvider, ReportProvider};
use agent_monitor::{Collector, ConnectionsReport, MonitorOptions};
use tempfile::TempDir;

const STAT: &str = "\
cpu  100 0 50 1000 0 0 0 0 0 0
cpu0 50 0 25 500 0 0 0 0 0 0
cpu1 50 0 25 500 0 0 0 0 0 0
";

const MEMINFO: &str = "\
MemTotal:      100000 kB
MemFree:        20000 kB
MemAvailable:   40000 kB
Buffers:         5000 kB
Cached:         10000 kB
SwapTotal:      10000 kB
SwapFree:        4000 kB
SReclaimable:    3000 kB
Shmem:           1000 kB
";

fn proc_net_dev(eth0_rx: u64, eth0_tx: u64) -> String {
    format!(
        "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 999999 1 0 0 0 0 0 0 999999 1 0 0 0 0 0 0
  eth0: {eth0_rx} 10 0 0 0 0 0 0 {eth0_tx} 10 0 0 0 0 0 0
docker0: 5000 5 0 0 0 0 0 0 5000 5 0 0 0 0 0 0
"
    )
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temp dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture dir");
        }
        fs::write(&path, content).expect("write fixture");
        path
    }

    fn options(&self, month_rotate: i64) -> MonitorOptions {
        MonitorOptions::from_config(self.path().to_str().unwrap(), false, false)
            .with_sys_root(self.path())
            .with_cpu_sample_interval(Duration::from_millis(1))
            .with_network_sample_interval(Duration::from_millis(1))
            .with_net_static_path(self.path().join("net_static.json"))
            .with_filters("", "", "", month_rotate)
            .with_gpu_tools(
                self.path().join("no-nvidia-smi"),
                self.path().join("no-rocm-smi"),
                Some(self.path().join("empty-path").into_os_string()),
            )
    }

    fn populate(&self) {
        self.write("stat", STAT);
        self.write("meminfo", MEMINFO);
        self.write("loadavg", "0.1 0.2 0.3 1/10 42\n");
        self.write("uptime", "100.5 200.0\n");
        self.write("self/mounts", "/dev/sda1 / ext4 rw 0 0\n");
        self.write("net/dev", &proc_net_dev(5000, 4000));
        self.write(
            "net/tcp",
            "  sl  local_address rem_address st\n   0: row\n   1: row\n",
        );
        self.write("net/tcp6", "  sl\n   0: row\n");
        self.write("net/udp", "  sl\n   0: row\n");
        self.write("net/udp6", "  sl\n");
        for name in ["1", "42", "4242", "self", "net", "sys"] {
            fs::create_dir_all(self.path().join(name)).unwrap();
        }
    }
}

fn provider(fixture: &Fixture, month_rotate: i64) -> (Arc<Collector>, MonitorReportProvider) {
    let collector = Arc::new(Collector::new(fixture.options(month_rotate)));
    let provider = MonitorReportProvider::new(
        collector.clone(),
        IpOptions::default(),
        Duration::from_millis(30),
        "test",
    );
    (collector, provider)
}

#[test]
fn collect_report_passes_a03_metrics_through() {
    let fixture = Fixture::new();
    fixture.populate();
    let (collector, provider) = provider(&fixture, 0);

    let started = std::time::Instant::now();
    let report = provider.collect_report();
    let elapsed = started.elapsed();

    assert_eq!(report.disk, collector.collect_disk());
    assert_eq!(report.network.total_up, 4000);
    assert_eq!(report.network.total_down, 5000);
    assert_eq!(report.network.up, 0);
    assert_eq!(report.network.down, 0);
    assert_eq!(report.connections, ConnectionsReport { tcp: 3, udp: 1 });
    assert_eq!(report.process, 3);
    assert_eq!(report.uptime, 100);
    assert!(report.gpu.is_none());
    assert_eq!(report.message, "");
    // Static /proc/stat counters produce zero usage; the Go floor applies.
    assert_eq!(report.cpu_usage, 0.001);
    // The 30ms fixed CPU window is honoured by the provider.
    assert!(
        elapsed >= Duration::from_millis(25),
        "CPU window was not used: {elapsed:?}"
    );
}

#[test]
fn month_rotate_totals_come_from_a03_netstatic() {
    let fixture = Fixture::new();
    fixture.populate();
    let (_collector, provider) = provider(&fixture, 1);

    let first = provider.collect_report();
    assert_eq!(first.network.total_up, 0);
    assert_eq!(first.network.total_down, 0);

    fixture.write("net/dev", &proc_net_dev(5100, 4200));
    let second = provider.collect_report();
    assert_eq!(second.network.total_down, 100);
    assert_eq!(second.network.total_up, 200);
}
