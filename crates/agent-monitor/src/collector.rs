//! Stateful collector.

use parking_lot::Mutex;

use crate::cpu;
use crate::load;
use crate::memory;
use crate::options::MonitorOptions;
use crate::os;
use crate::procfs::ProcStat;
use crate::types::{
    BasicInfo, CpuInfo, CpuReport, LoadAverage, MemoryInfo, OsInfo, RamReport, ReportMetrics,
    SwapInfo, SystemSnapshot,
};
use crate::uptime;
use crate::virtualization;

/// Collects the basic system metrics owned by this crate.
///
/// The collector caches the previous `/proc/stat` sample so repeated calls can
/// compute CPU usage without sleeping every time. All state is behind a mutex,
/// so a single collector can be shared via `Arc` by the reporting loop.
#[derive(Debug)]
pub struct Collector {
    options: MonitorOptions,
    last_cpu_stat: Mutex<Option<ProcStat>>,
}

impl Collector {
    pub fn new(options: MonitorOptions) -> Self {
        Self {
            options,
            last_cpu_stat: Mutex::new(None),
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
    /// CPU is sampled first; on the first call this may block for
    /// [`MonitorOptions::cpu_sample_interval`] (one second by default).
    pub fn collect(&self) -> SystemSnapshot {
        SystemSnapshot {
            cpu: self.collect_cpu(),
            memory: self.collect_memory(),
            swap: self.collect_swap(),
            load: self.collect_load(),
            os: self.collect_os(),
            uptime: self.collect_uptime(),
            virtualization: self.collect_virtualization(),
        }
    }

    /// Collect the periodic report payload (`cpu`, `ram`, `swap`, `load`,
    /// `uptime`) without OS or virtualization probing.
    ///
    /// The CPU usage floor matches Go's `GenerateReport`: values at or below
    /// `0.001` are reported as `0.001` so an idle machine never reports exactly
    /// zero.
    pub fn collect_report_metrics(&self) -> ReportMetrics {
        let cpu = self.collect_cpu();
        let memory = self.collect_memory();
        let swap = self.collect_swap();
        let load = self.collect_load();
        let uptime = self.collect_uptime();

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
            uptime,
        }
    }

    pub fn collect_cpu(&self) -> CpuInfo {
        cpu::collect(&self.options, &self.last_cpu_stat)
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
            virtualization: self.collect_virtualization(),
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new(MonitorOptions::default())
    }
}
