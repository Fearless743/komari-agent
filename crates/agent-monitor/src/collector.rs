//! Stateful collector.

use parking_lot::Mutex;

use crate::cpu;
use crate::load;
use crate::memory;
use crate::options::MonitorOptions;
use crate::os;
use crate::procfs::ProcStat;
use crate::types::{BasicInfo, CpuInfo, LoadAverage, MemoryInfo, OsInfo, SwapInfo, SystemSnapshot};
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
