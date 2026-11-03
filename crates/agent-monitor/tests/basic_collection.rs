//! Fixture-based and real-environment tests for the basic collector.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_monitor::{
    Collector, MemoryMode, MonitorOptions, parse_loadavg, parse_meminfo, parse_proc_stat,
    parse_uptime,
};
use tempfile::TempDir;

const KIB: u64 = 1024;

const STAT: &str = "\
cpu  100 0 50 1000 0 0 0 0 0 0
cpu0 50 0 25 500 0 0 0 0 0 0
cpu1 50 0 25 500 0 0 0 0 0 0
intr 12345 0 0
ctxt 67890
";

const MEMINFO: &str = "\
MemTotal:      100000 kB
MemFree:        20000 kB
MemAvailable:   40000 kB
Buffers:         5000 kB
Cached:         10000 kB
SwapCached:      1000 kB
SwapTotal:      10000 kB
SwapFree:        4000 kB
Shmem:           1000 kB
SReclaimable:    3000 kB
Zswap:              0 kB
Zswapped:           0 kB
";

const CPUINFO: &str = "\
processor\t: 0
model name\t: Fixture CPU v1
physical id\t: 0
core id\t\t: 0

processor\t: 1
model name\t: Fixture CPU v1
physical id\t: 0
core id\t\t: 1

";

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn empty() -> Self {
        Self {
            dir: TempDir::new().expect("temp dir"),
        }
    }

    fn full() -> Self {
        let fixture = Self::empty();
        fixture.write("stat", STAT);
        fixture.write("meminfo", MEMINFO);
        fixture.write("loadavg", "0.52 0.58 0.59 1/1234 56789\n");
        fixture.write("uptime", "12345.67 98765.43\n");
        fixture.write("cpuinfo", CPUINFO);
        fixture.write("sys/kernel/osrelease", "6.6.6-fixture\n");
        fixture
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.dir.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture directory");
        }
        fs::write(path, content).expect("write fixture");
    }

    fn options(&self) -> MonitorOptions {
        MonitorOptions::from_config(self.path().to_str().expect("utf-8 temp path"), false, false)
            .with_cpu_sample_interval(Duration::from_millis(1))
    }

    fn collector(&self) -> Collector {
        Collector::new(self.options())
    }
}

fn kib(value: u64) -> u64 {
    value * KIB
}

#[test]
fn host_proc_fixture_drives_every_proc_reader() {
    let fixture = Fixture::full();
    let snapshot = fixture.collector().collect();

    assert_eq!(snapshot.cpu.name, "Fixture CPU v1");
    assert_eq!(snapshot.cpu.cores, 2);
    assert_eq!(snapshot.cpu.physical_cores, 2);
    assert_eq!(snapshot.cpu.usage, 0.0);
    assert_eq!(snapshot.cpu.per_core_usage, vec![0.0, 0.0]);

    assert_eq!(snapshot.memory.total, kib(100000));
    assert_eq!(snapshot.memory.available, kib(40000));
    assert_eq!(snapshot.memory.free, kib(20000));
    assert_eq!(snapshot.memory.used, kib(63000));
    assert_eq!(snapshot.memory.mode, MemoryMode::HtopLike);

    assert_eq!(snapshot.swap.total, kib(10000));
    assert_eq!(snapshot.swap.used, kib(5000));

    assert_eq!(snapshot.load.load1, 0.52);
    assert_eq!(snapshot.load.load5, 0.58);
    assert_eq!(snapshot.load.load15, 0.59);
    assert_eq!(snapshot.uptime, 12345);
    assert_eq!(snapshot.os.kernel_version, "6.6.6-fixture");
    assert!(!snapshot.os.arch.is_empty());
    assert!(!snapshot.os.name.is_empty());
}

#[test]
fn host_proc_env_lookup_is_injected_without_global_state() {
    let fixture = Fixture::full();
    let host_proc = fixture.path().to_string_lossy().into_owned();
    let lookup = |key: &str| (key == "HOST_PROC").then(|| host_proc.clone());

    let options = MonitorOptions::from_env_lookup(&lookup);
    assert_eq!(options.proc_root, fixture.path());

    let snapshot =
        Collector::new(options.with_cpu_sample_interval(Duration::from_millis(1))).collect();
    assert_eq!(snapshot.memory.total, kib(100000));
    assert_eq!(snapshot.uptime, 12345);

    let empty = MonitorOptions::from_env_lookup(&|_| None);
    assert_eq!(empty.proc_root, PathBuf::from("/proc"));
    let blank = MonitorOptions::from_env_lookup(&|_| Some("   ".to_owned()));
    assert_eq!(blank.proc_root, PathBuf::from("/proc"));
}

#[test]
fn memory_modes_match_go_accounting() {
    let fixture = Fixture::full();

    let default = fixture.collector().collect_memory();
    assert_eq!(default.mode, MemoryMode::HtopLike);
    assert_eq!(default.used, kib(63000));

    let include_cache = Collector::new(
        MonitorOptions::from_config(fixture.path().to_str().unwrap(), true, false)
            .with_cpu_sample_interval(Duration::from_millis(1)),
    )
    .collect_memory();
    assert_eq!(include_cache.mode, MemoryMode::IncludeCache);
    assert_eq!(include_cache.used, kib(80000));

    let raw_used = Collector::new(
        MonitorOptions::from_config(fixture.path().to_str().unwrap(), false, true)
            .with_cpu_sample_interval(Duration::from_millis(1)),
    )
    .collect_memory();
    assert_eq!(raw_used.mode, MemoryMode::HtopLike);
    assert_eq!(raw_used.used, kib(63000));

    // Go checks memory_include_cache first, so it wins when both are set.
    let both = Collector::new(
        MonitorOptions::from_config(fixture.path().to_str().unwrap(), true, true)
            .with_cpu_sample_interval(Duration::from_millis(1)),
    )
    .collect_memory();
    assert_eq!(both.mode, MemoryMode::IncludeCache);
    assert_eq!(both.used, kib(80000));
}

#[test]
fn cpu_usage_uses_delta_between_cached_samples() {
    let fixture = Fixture::full();
    let collector = fixture.collector();

    let first = collector.collect_cpu();
    assert_eq!(
        first.usage, 0.0,
        "two reads of the same fixture have no delta"
    );

    fixture.write(
        "stat",
        "\
cpu  130 0 50 1070 0 0 0 0 0 0
cpu0 65 0 25 535 0 0 0 0 0 0
cpu1 65 0 25 535 0 0 0 0 0 0
",
    );
    let second = collector.collect_cpu();
    assert!((second.usage - 30.0).abs() < 0.001, "{:?}", second.usage);
    assert_eq!(second.per_core_usage.len(), 2);
    assert!((second.per_core_usage[0] - 30.0).abs() < 0.001);

    fixture.write(
        "stat",
        "\
cpu  230 0 50 1070 0 0 0 0 0 0
cpu0 115 0 25 535 0 0 0 0 0 0
cpu1 115 0 25 535 0 0 0 0 0 0
",
    );
    let busy = collector.collect_cpu();
    assert!((busy.usage - 100.0).abs() < 0.001, "{:?}", busy.usage);

    // Counters moving backwards must clamp to zero instead of underflowing.
    fixture.write(
        "stat",
        "\
cpu  10 0 5 20 0 0 0 0 0 0
cpu0 5 0 2 10 0 0 0 0 0 0
cpu1 5 0 2 10 0 0 0 0 0 0
",
    );
    assert_eq!(collector.collect_cpu().usage, 0.0);
}

#[test]
fn meminfo_parsing_handles_units_and_underflow() {
    let info = parse_meminfo(MEMINFO);
    assert_eq!(info.mem_total, kib(100000));
    assert_eq!(info.mem_available, kib(40000));
    assert_eq!(info.swap_cached, kib(1000));
    assert_eq!(info.htop_used(), kib(63000));
    assert_eq!(info.available(), kib(40000));

    // MemAvailable missing: fall back to total - free - buffers - cached.
    let old_kernel =
        parse_meminfo("MemTotal: 1000 kB\nMemFree: 500 kB\nBuffers: 100 kB\nCached: 200 kB\n");
    assert_eq!(old_kernel.available(), kib(200));

    // Reclaimable values larger than the total must not underflow.
    let broken = parse_meminfo(
        "MemTotal: 1000 kB\nMemFree: 800 kB\nBuffers: 500 kB\nCached: 500 kB\n\
         SReclaimable: 500 kB\nShmem: 100 kB\n",
    );
    assert_eq!(broken.htop_used(), kib(300));

    // Malformed and unknown lines are ignored.
    let malformed = parse_meminfo("MemTotal: not-a-number\nBogus: 12 kB\nMemFree: 42 kB\n");
    assert_eq!(malformed.mem_total, 0);
    assert_eq!(malformed.mem_free, kib(42));
}

#[test]
fn swap_underflow_uses_free_fallback() {
    let fixture = Fixture::empty();
    fixture.write(
        "meminfo",
        "MemTotal: 1000 kB\nSwapTotal: 1000 kB\nSwapFree: 900 kB\nSwapCached: 500 kB\n",
    );

    let swap = fixture.collector().collect_swap();
    assert_eq!(swap.total, kib(1000));
    assert_eq!(swap.used, kib(100));
}

#[test]
fn proc_text_parsers_reject_malformed_input() {
    assert!(parse_proc_stat("intr 1 2 3\n").is_err());
    assert!(parse_proc_stat("cpu 1 2\n").is_err());

    let stat =
        parse_proc_stat("cpu 1 2 3 4 5 6 7 8 9 10\ncpu0 1 2 3 4\ncpu1 5 6 7 8\nweird 0 0 0 0\n")
            .unwrap();
    assert_eq!(stat.cores.len(), 2);
    assert_eq!(stat.aggregate.total(), 36);
    assert_eq!(stat.aggregate.idle_total(), 9);

    let load = parse_loadavg("1.5 2.5 3.5 2/100 4242\n").unwrap();
    assert_eq!((load.load1, load.load5, load.load15), (1.5, 2.5, 3.5));
    assert!(parse_loadavg("1.0 2.0").is_err());
    assert!(parse_loadavg("a b c").is_err());
    assert!(parse_loadavg("NaN 1 2").is_err());

    assert_eq!(parse_uptime("42.9 100.0\n").unwrap(), 42);
    assert!(parse_uptime("").is_err());
    assert!(parse_uptime("-1 100").is_err());
    assert!(parse_uptime("nope 1").is_err());
}

#[test]
fn usage_between_clamps_and_matches_per_core() {
    let previous = parse_proc_stat("cpu 100 0 0 100 0 0 0 0\ncpu0 50 0 0 50 0 0 0 0\n").unwrap();
    let current = parse_proc_stat("cpu 150 0 0 150 0 0 0 0\ncpu0 75 0 0 75 0 0 0 0\n").unwrap();

    let usage = agent_monitor::usage_between(&previous, &current);
    assert!((usage.total - 50.0).abs() < 0.001);
    assert_eq!(usage.per_core, vec![50.0]);

    let reset = agent_monitor::usage_between(&current, &previous);
    assert_eq!(reset.total, 0.0);
    assert_eq!(reset.per_core, vec![0.0]);
}

#[test]
fn missing_proc_files_fall_back_gracefully() {
    let fixture = Fixture::empty();
    let collector = fixture.collector();
    let snapshot = collector.collect();

    assert!(snapshot.cpu.cores > 0);
    assert!(!snapshot.cpu.name.is_empty());
    assert!(snapshot.uptime < u64::MAX);
    assert!(snapshot.load.load1.is_finite() && snapshot.load.load1 >= 0.0);
    assert!(!snapshot.os.name.is_empty());
    assert!(!snapshot.os.arch.is_empty());
    // `sysinfo` still provides memory on the host even when the injected proc
    // root is empty; it may legitimately be zero in very restricted sandboxes.
    assert!(snapshot.memory.total < u64::MAX);
}

#[test]
fn real_environment_smoke_test() {
    if !Path::new("/proc/stat").exists() {
        eprintln!("skipping: /proc/stat is not available");
        return;
    }

    let collector = Collector::new(
        MonitorOptions::default().with_cpu_sample_interval(Duration::from_millis(50)),
    );
    let snapshot = collector.collect();

    assert!(snapshot.cpu.cores > 0);
    assert!(!snapshot.cpu.name.is_empty());
    assert!(snapshot.cpu.usage >= 0.0 && snapshot.cpu.usage <= 100.0);
    assert!(snapshot.uptime < u64::MAX);
    assert!(snapshot.memory.total > 0);
    assert!(snapshot.memory.used <= snapshot.memory.total);
    assert!(snapshot.memory.available <= snapshot.memory.total);
    assert!(!snapshot.os.name.is_empty());
    assert!(!snapshot.os.kernel_version.is_empty());
    assert!(!snapshot.os.arch.is_empty());
}

#[test]
fn basic_info_matches_go_field_semantics() {
    let fixture = Fixture::full();
    let collector = fixture.collector();
    let info = collector.collect_basic_info();

    assert_eq!(info.cpu_name, "Fixture CPU v1");
    assert_eq!(info.cpu_cores, 2);
    assert_eq!(info.cpu_physical_cores, 2);
    assert_eq!(info.arch, info.arch.to_ascii_lowercase());
    assert!(!info.os.is_empty());
    assert_eq!(info.kernel_version, "6.6.6-fixture");
    assert_eq!(info.mem_total, kib(100000));
    assert_eq!(info.swap_total, kib(10000));

    let json = serde_json::to_value(&info).expect("serialize basic info");
    for key in [
        "cpu_name",
        "cpu_cores",
        "cpu_physical_cores",
        "arch",
        "os",
        "kernel_version",
        "mem_total",
        "swap_total",
        "virtualization",
    ] {
        assert!(json.get(key).is_some(), "missing basic info key {key}");
    }
}

#[test]
fn snapshot_serializes_with_report_key_names() {
    let fixture = Fixture::full();
    let snapshot = fixture.collector().collect();
    let json = serde_json::to_value(&snapshot).expect("serialize snapshot");

    assert_eq!(json["ram"]["total"], kib(100000));
    assert_eq!(json["swap"]["used"], kib(5000));
    assert_eq!(json["load"]["load1"], 0.52);
    assert_eq!(json["cpu"]["cpu_name"], "Fixture CPU v1");
    assert!(json["uptime"].as_u64().unwrap() == 12345);
}
