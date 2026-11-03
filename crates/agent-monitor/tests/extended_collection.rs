//! Fixture-based tests for disk, network, connections, process and GPU
//! collection added by A03.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_monitor::{
    Collector, ConnectionsCount, GpuReport, MonitorOptions, NetStaticStore, NetworkReport,
    parse_mounts, safe_delta,
};
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

    fn base_options(&self) -> MonitorOptions {
        MonitorOptions::from_config(self.path().to_str().unwrap(), false, false)
            .with_sys_root(self.path())
            .with_cpu_sample_interval(Duration::from_millis(1))
            .with_network_sample_interval(Duration::from_millis(1))
            .with_net_static_path(self.path().join("net_static.json"))
            .with_gpu_tools(
                self.path().join("no-nvidia-smi"),
                self.path().join("no-rocm-smi"),
                Some(self.path().join("empty-path").into_os_string()),
            )
    }

    fn collector(&self) -> Collector {
        Collector::new(self.base_options())
    }

    #[cfg(unix)]
    fn executable(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = self.write(name, body);
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

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

fn write_proc_net_files(fixture: &Fixture) {
    fixture.write(
        "net/tcp",
        "  sl  local_address rem_address st\n   0: row\n   1: row\n",
    );
    fixture.write("net/tcp6", "  sl\n   0: row\n");
    fixture.write("net/udp", "  sl\n   0: row\n");
    fixture.write("net/udp6", "  sl\n");
}

fn write_process_dirs(fixture: &Fixture) {
    for name in ["1", "42", "4242", "self", "net", "sys"] {
        fs::create_dir_all(fixture.path().join(name)).unwrap();
    }
}

#[cfg(unix)]
fn fake_nvidia_smi(fixture: &Fixture) -> PathBuf {
    fixture.executable(
        "bin/nvidia-smi",
        "#!/bin/sh\ncat <<'XML'\n<nvidia_smi_log>\n  <gpu>\n    <product_name>NVIDIA GeForce RTX 4090</product_name>\n    <fb_memory_usage><total>24564 MiB</total><used>1024 MiB</used></fb_memory_usage>\n    <utilization><gpu_util>42 %</gpu_util></utilization>\n    <temperature><gpu_temp>61 C</gpu_temp></temperature>\n  </gpu>\n</nvidia_smi_log>\nXML\n",
    )
}

#[test]
fn include_mountpoints_sums_only_the_listed_paths() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().join("mnt/a")).unwrap();
    fs::create_dir_all(fixture.path().join("mnt/b")).unwrap();
    let a = fixture.path().join("mnt/a");
    let b = fixture.path().join("mnt/b");
    let missing = fixture.path().join("mnt/missing");

    let only_a = Collector::new(fixture.base_options().with_filters(
        a.to_str().unwrap(),
        "",
        "",
        0,
    ))
    .collect_disk();
    let only_b = Collector::new(fixture.base_options().with_filters(
        b.to_str().unwrap(),
        "",
        "",
        0,
    ))
    .collect_disk();
    let both = Collector::new(fixture.base_options().with_filters(
        format!("{}; {}; {}", a.display(), b.display(), missing.display()),
        "",
        "",
        0,
    ))
    .collect_disk();

    assert!(only_a.total > 0);
    // statvfs totals can shift by a block while the fixture directory is
    // touched, so assert the aggregation invariant instead of exact bytes.
    assert!(both.total > only_a.total);
    assert!(both.total >= only_b.total);
    assert!(both.used >= only_a.used);
}

#[test]
fn injected_mount_table_drives_disk_collection_and_skips_pseudo_fs() {
    let fixture = Fixture::new();
    fixture.write(
        "mounts",
        "/dev/sda1 / ext4 rw,relatime 0 0\n\
         tmpfs /tmp tmpfs rw 0 0\n\
         proc /proc proc rw 0 0\n\
         /dev/loop0 /snap/core squashfs ro 0 0\n\
         /dev/missing /mnt/missing ext4 rw 0 0\n",
    );

    let disk = fixture.collector().collect_disk();
    assert!(disk.total > 0);
    assert!(disk.used <= disk.total);

    let parsed = parse_mounts(&fs::read_to_string(fixture.path().join("mounts")).unwrap());
    assert_eq!(parsed.len(), 5);
}

#[test]
fn network_report_uses_selected_interfaces_and_skips_loopback() {
    let fixture = Fixture::new();
    fixture.write("net/dev", &proc_net_dev(5000, 4000));

    let report = fixture.collector().collect_network();
    assert_eq!(report.total_down, 5000);
    assert_eq!(report.total_up, 4000);
    assert_eq!(report.up, 0);
    assert_eq!(report.down, 0);

    let selected = Collector::new(fixture.base_options().with_filters("", "eth*", "eth1", 0))
        .collect_network();
    assert_eq!(selected.total_down, 5000);
    assert_eq!(selected.total_up, 4000);

    let excluded =
        Collector::new(fixture.base_options().with_filters("", "", "eth*", 0)).collect_network();
    assert_eq!(excluded, NetworkReport::default());
}

#[test]
fn month_rotate_reports_traffic_since_reset_and_persists_it() {
    let fixture = Fixture::new();
    fixture.write("net/dev", &proc_net_dev(1000, 2000));

    let collector = Collector::new(fixture.base_options().with_filters("", "eth*", "", 1));
    let first = collector.collect_network();
    assert_eq!(first.total_down, 0);
    assert_eq!(first.total_up, 0);

    fixture.write("net/dev", &proc_net_dev(1500, 2600));
    let second = collector.collect_network();
    assert_eq!(second.total_down, 500);
    assert_eq!(second.total_up, 600);

    let database = fixture.path().join("net_static.json");
    assert!(database.is_file(), "monthly database should be persisted");
    let store = NetStaticStore::load(&database);
    assert_eq!(store.totals_between(0, 0)["eth0"].rx, 500);
    assert_eq!(store.totals_between(0, 0)["eth0"].tx, 600);

    let loaded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&database).unwrap()).unwrap();
    assert!(loaded["interfaces"]["eth0"].is_array());
}

#[test]
fn connections_and_process_counts_honour_host_proc() {
    let fixture = Fixture::new();
    write_proc_net_files(&fixture);
    write_process_dirs(&fixture);

    let collector = fixture.collector();
    assert_eq!(
        collector.collect_connections(),
        ConnectionsCount { tcp: 3, udp: 1 }
    );
    assert_eq!(collector.collect_process_count(), 3);
}

#[test]
fn empty_proc_root_degrades_all_new_metrics_to_zero() {
    let fixture = Fixture::new();
    let report = fixture.collector().collect_report_metrics();

    assert_eq!(report.disk.total, 0);
    assert_eq!(report.network, NetworkReport::default());
    assert_eq!(report.connections, Default::default());
    assert_eq!(report.process, 0);
    assert!(report.gpu.is_none());
    assert!(report.message.contains("network speed"));
    assert!(report.message.contains("failed to get connections"));
}

#[test]
fn report_wire_shape_contains_go_fields_and_types() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().join("mnt/data")).unwrap();
    let data = fixture.path().join("mnt/data");
    fixture.write(
        "mounts",
        &format!("/dev/sda1 {} ext4 rw 0 0\n", data.display()),
    );
    fixture.write("net/dev", &proc_net_dev(5000, 4000));
    write_proc_net_files(&fixture);
    write_process_dirs(&fixture);
    fixture.write("stat", STAT);
    fixture.write("meminfo", MEMINFO);
    fixture.write("loadavg", "0.1 0.2 0.3 1/10 42\n");
    fixture.write("uptime", "100.5 200.0\n");

    let json = serde_json::to_value(fixture.collector().collect_report_metrics()).unwrap();
    let object = json.as_object().unwrap();
    for key in [
        "cpu",
        "ram",
        "swap",
        "load",
        "disk",
        "network",
        "connections",
        "uptime",
        "process",
        "message",
    ] {
        assert!(object.contains_key(key), "missing {key}");
    }
    assert!(object.get("gpu").is_none());

    assert!(json["disk"]["total"].is_u64());
    assert!(json["disk"]["used"].is_u64());
    assert!(json["network"]["up"].is_u64());
    assert!(json["network"]["down"].is_u64());
    assert!(json["network"]["totalUp"].is_u64());
    assert!(json["network"]["totalDown"].is_u64());
    assert_eq!(json["connections"]["tcp"], 3);
    assert_eq!(json["connections"]["udp"], 1);
    assert_eq!(json["process"], 3);
    assert_eq!(json["uptime"], 100);
    assert_eq!(json["message"], "");
}

#[cfg(unix)]
#[test]
fn gpu_switch_off_is_absent_and_fake_nvidia_smi_produces_detailed_info() {
    let fixture = Fixture::new();
    let nvidia = fake_nvidia_smi(&fixture);

    let without = serde_json::to_value(fixture.collector().collect_report_metrics()).unwrap();
    assert!(without.get("gpu").is_none());

    let options = fixture
        .base_options()
        .with_gpu_enabled(true)
        .with_gpu_tools(
            nvidia.clone(),
            fixture.path().join("no-rocm"),
            Some(fixture.path().join("empty-path").into_os_string()),
        );
    let report = Collector::new(options).collect_report_metrics();
    let gpu = report.gpu.as_ref().expect("detailed gpu report");
    assert_eq!(gpu.count, Some(1));
    assert_eq!(gpu.average_usage, Some(42.0));
    assert_eq!(
        gpu.detailed_info.as_ref().unwrap()[0].name,
        "NVIDIA GeForce RTX 4090"
    );

    let json = serde_json::to_value(report).unwrap();
    assert_eq!(json["gpu"]["count"], 1);
    assert_eq!(json["gpu"]["average_usage"], 42.0);
    assert_eq!(
        json["gpu"]["detailed_info"][0]["memory_total"],
        24564u64 * 1024 * 1024
    );
    assert_eq!(json["gpu"]["detailed_info"][0]["temperature"], 61);
}

#[test]
fn models_only_report_serializes_the_fallback_shape() {
    let report = GpuReport::models(vec![
        "NVIDIA GeForce RTX 4090".to_owned(),
        "AMD Radeon RX 7900 XTX".to_owned(),
    ])
    .unwrap();
    let json = serde_json::to_value(report).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "models": ["NVIDIA GeForce RTX 4090", "AMD Radeon RX 7900 XTX"]
        })
    );
}

#[cfg(target_os = "linux")]
#[test]
fn sysfs_drm_fixture_provides_the_basic_gpu_name() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let driver = fixture.path().join("drivers/i915");
    fs::create_dir_all(&driver).unwrap();
    let device = fixture.path().join("class/drm/card0/device");
    fs::create_dir_all(&device).unwrap();
    symlink(&driver, device.join("driver")).unwrap();

    let name = fixture.collector().collect_gpu_name();
    assert_eq!(name, "Intel Integrated Graphics");
}

#[test]
fn basic_info_includes_disk_total_and_gpu_name() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().join("mnt/data")).unwrap();
    let data = fixture.path().join("mnt/data");
    fixture.write(
        "mounts",
        &format!("/dev/sda1 {} ext4 rw 0 0\n", data.display()),
    );
    fixture.write("stat", STAT);
    fixture.write("meminfo", MEMINFO);

    let collector = Collector::new(fixture.base_options().with_filters(
        data.to_str().unwrap(),
        "",
        "",
        0,
    ));
    let info = collector.collect_basic_info();
    assert!(info.disk_total > 0);
    assert_eq!(info.gpu_name, "None");

    let json = serde_json::to_value(info).unwrap();
    assert!(json["disk_total"].is_u64());
    assert!(json["gpu_name"].is_string());
}

#[test]
fn netstatic_safe_delta_never_underflows() {
    assert_eq!(safe_delta(10, 20), 0);
    assert_eq!(safe_delta(20, 10), 10);
}
