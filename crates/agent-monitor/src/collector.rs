//! Stateful collector.

use parking_lot::Mutex;

use crate::connections;
use crate::cpu;
use crate::disk;
use crate::gpu;
use crate::load;
use crate::memory;
use crate::net;
use crate::netstatic::NetStaticStore;
use crate::options::MonitorOptions;
use crate::os;
use crate::process;
use crate::procfs::ProcStat;
use crate::types::{
    BasicInfo, ConnectionsCount, ConnectionsReport, CpuInfo, CpuReport, DiskInfo, GpuReport,
    LoadAverage, MemoryInfo, NetworkReport, OsInfo, RamReport, ReportMetrics, SwapInfo,
    SystemSnapshot,
};
use crate::uptime;
use crate::virtualization;

/// Collects the system metrics owned by this crate.
///
/// The collector caches the previous `/proc/stat` sample so repeated calls can
/// compute CPU usage without sleeping every time, and the monthly traffic
/// database is reused across calls. All state is behind a mutex, so a single
/// collector can be shared via `Arc` by the reporting loop.
#[derive(Debug)]
pub struct Collector {
    options: MonitorOptions,
    last_cpu_stat: Mutex<Option<ProcStat>>,
    net_static: Mutex<Option<NetStaticStore>>,
}

impl Collector {
    pub fn new(options: MonitorOptions) -> Self {
        Self {
            options,
            last_cpu_stat: Mutex::new(None),
            net_static: Mutex::new(None),
        }
    }

    /// Options used by this collector.
    pub fn options(&self) -> &MonitorOptions {
        &self.options
    }

    /// Collect every metric in this crate's scope.
    ///
    /// This is a full diagnostic snapshot and includes OS/virtualization
    /// probes. Reporting loops should use [`Collector::collect_report_metrics`]
    /// for periodic samples and [`Collector::collect_basic_info`] on the slower
    /// basic-info schedule.
    ///
    /// CPU and network sampling may each block for their configured interval
    /// (one second by default) on the first call.
    pub fn collect(&self) -> SystemSnapshot {
        let (network, _) = net::collect(&self.options, &self.net_static);
        let (gpu_report, _) = gpu::collect_report(&self.options);

        SystemSnapshot {
            cpu: self.collect_cpu(),
            memory: self.collect_memory(),
            swap: self.collect_swap(),
            load: self.collect_load(),
            disk: self.collect_disk(),
            network,
            connections: self.collect_connections().into(),
            process: self.collect_process_count(),
            gpu_name: self.collect_gpu_name(),
            gpu: gpu_report,
            os: self.collect_os(),
            uptime: self.collect_uptime(),
            virtualization: self.collect_virtualization(),
        }
    }

    /// Collect the periodic report payload without OS or virtualization
    /// probing.
    ///
    /// The CPU usage floor matches Go's `GenerateReport`: values at or below
    /// `0.001` are reported as `0.001` so an idle machine never reports exactly
    /// zero. Network and connection failures accumulate into
    /// [`ReportMetrics::message`] just like the Go agent.
    pub fn collect_report_metrics(&self) -> ReportMetrics {
        let cpu = self.collect_cpu();
        let memory = self.collect_memory();
        let swap = self.collect_swap();
        let load = self.collect_load();
        let disk = self.collect_disk();
        let (network, network_error) = net::collect(&self.options, &self.net_static);
        let (connections, connections_error) = match connections::collect(&self.options) {
            Ok(count) => (ConnectionsReport::from(count), None),
            Err(error) => (
                ConnectionsReport::default(),
                Some(format!("failed to get connections: {error}\n")),
            ),
        };
        let uptime = self.collect_uptime();
        let process = self.collect_process_count();
        let (gpu_report, gpu_error) = gpu::collect_report(&self.options);

        let mut message = network_error.unwrap_or_default();
        if let Some(error) = connections_error {
            message.push_str(&error);
        }
        if let Some(error) = gpu_error {
            message.push_str(&error);
        }

        ReportMetrics {
            cpu: CpuReport {
                usage: if cpu.usage <= 0.001 { 0.001 } else { cpu.usage },
            },
            ram: RamReport {
                total: memory.total,
                used: memory.used,
            },
            swap: RamReport {
                total: swap.total,
                used: swap.used,
            },
            load,
            disk,
            network,
            connections,
            gpu: gpu_report,
            uptime,
            process,
            message,
        }
    }

    pub fn collect_cpu(&self) -> CpuInfo {
        cpu::collect(&self.options, &self.last_cpu_stat)
    }

    /// Force a fresh CPU measurement over exactly `window`.
    ///
    /// Use this when A05 needs a fixed reporting window (Go samples for one
    /// second every call). [`Collector::collect_cpu`] instead measures the time
    /// elapsed since the previous call and never blocks after the first sample.
    pub fn sample_cpu(&self, window: std::time::Duration) -> CpuInfo {
        cpu::sample(&self.options, &self.last_cpu_stat, window)
    }

    pub fn collect_memory(&self) -> MemoryInfo {
        memory::collect(&self.options)
    }

    pub fn collect_swap(&self) -> SwapInfo {
        memory::collect_swap(&self.options)
    }

    pub fn collect_load(&self) -> LoadAverage {
        load::collect(&self.options)
    }

    pub fn collect_disk(&self) -> DiskInfo {
        disk::collect(&self.options)
    }

    /// Collect network metrics. Failures degrade to zeroes; use
    /// [`Collector::collect_report_metrics`] to also receive the diagnostic
    /// message.
    pub fn collect_network(&self) -> NetworkReport {
        net::collect(&self.options, &self.net_static).0
    }

    /// Collect TCP/UDP socket counts. Failures degrade to zeroes.
    pub fn collect_connections(&self) -> ConnectionsCount {
        connections::collect(&self.options).unwrap_or_default()
    }

    pub fn collect_process_count(&self) -> i64 {
        process::collect(&self.options)
    }

    /// Detect the basic GPU model string (`None` / `Unknown` when absent).
    pub fn collect_gpu_name(&self) -> String {
        gpu::name::detect(&self.options)
    }

    /// Collect the detailed GPU report when `enable_gpu` is set.
    pub fn collect_gpu(&self) -> Option<GpuReport> {
        gpu::collect_report(&self.options).0
    }

    pub fn collect_os(&self) -> OsInfo {
        os::collect(&self.options)
    }

    pub fn collect_uptime(&self) -> u64 {
        uptime::collect(&self.options)
    }

    pub fn collect_virtualization(&self) -> String {
        virtualization::detect_virtualization(&self.options)
    }

    /// Collect the subset of data uploaded through the basic info request.
    pub fn collect_basic_info(&self) -> BasicInfo {
        let cpu = self.collect_cpu();
        let memory = self.collect_memory();
        let swap = self.collect_swap();
        let os = self.collect_os();

        BasicInfo {
            cpu_name: cpu.name,
            cpu_cores: cpu.cores,
            cpu_physical_cores: cpu.physical_cores,
            arch: cpu.arch,
            os: os.name,
            kernel_version: os.kernel_version,
            mem_total: memory.total,
            swap_total: swap.total,
            disk_total: self.collect_disk().total,
            gpu_name: self.collect_gpu_name(),
            virtualization: self.collect_virtualization(),
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new(MonitorOptions::default())
    }
}
