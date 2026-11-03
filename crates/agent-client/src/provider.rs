//! Metric provider abstraction.
//!
//! The reporting loop only needs "give me a report sample" and "give me a
//! basic info payload". Keeping that behind a trait lets the reconnect and
//! scheduling tests run without touching `/proc`, the network or a one second
//! CPU window.

use std::sync::Arc;
use std::time::Duration;

use agent_monitor::{BasicInfo, Collector};
use agent_protocol::BasicInfoV1;

use crate::basic_info::build_basic_info;
use crate::report::ReportSample;
use crate::supplement::SystemSupplement;

/// Source of report and basic-info data.
pub trait ReportProvider: Send + Sync {
    fn collect_report(&self) -> ReportSample;
    fn collect_basic_info(&self) -> BasicInfoV1;
}

/// Production provider backed by `agent-monitor` plus [`SystemSupplement`].
pub struct MonitorReportProvider {
    collector: Arc<Collector>,
    supplement: SystemSupplement,
    cpu_window: Duration,
    version: String,
}

impl MonitorReportProvider {
    pub fn new(
        collector: Arc<Collector>,
        supplement: SystemSupplement,
        cpu_window: Duration,
        version: impl Into<String>,
    ) -> Self {
        Self {
            collector,
            supplement,
            cpu_window,
            version: version.into(),
        }
    }

    pub fn collector(&self) -> &Arc<Collector> {
        &self.collector
    }
}

impl ReportProvider for MonitorReportProvider {
    fn collect_report(&self) -> ReportSample {
        // Take the first network sample before the CPU window and the second
        // one after it, so rates cover the real elapsed interval and the whole
        // report still costs roughly one second like Go.
        let started = std::time::Instant::now();
        let network_start = self.supplement.network_totals().ok();
        let cpu = self.collector.sample_cpu(self.cpu_window);
        let elapsed = started.elapsed();
        let extra = self.supplement.collect(network_start, elapsed);

        let memory = self.collector.collect_memory();
        let swap = self.collector.collect_swap();
        let load = self.collector.collect_load();
        let mut message = extra.message;

        // Go appends an uptime error to the report message. `collect_uptime`
        // hides the error behind its fallback, so probe once more to reproduce
        // the diagnostic (strict mode intentionally stays silent).
        #[cfg(target_os = "linux")]
        let uptime = match agent_monitor::read_uptime(&self.collector.options().proc_root) {
            Ok(uptime) => uptime,
            Err(error) => {
                if !self.collector.options().strict_proc {
                    message.push_str(&format!("failed to get uptime: {error}\n"));
                }
                self.collector.collect_uptime()
            }
        };
        #[cfg(not(target_os = "linux"))]
        let uptime = self.collector.collect_uptime();

        ReportSample {
            cpu_usage: cpu.usage,
            ram: agent_monitor::RamReport {
                total: memory.total,
                used: memory.used,
            },
            swap: agent_monitor::RamReport {
                total: swap.total,
                used: swap.used,
            },
            load,
            disk: extra.disk,
            network: extra.network,
            connections: extra.connections,
            uptime,
            process: extra.process,
            gpu: extra.gpu,
            message,
        }
    }

    fn collect_basic_info(&self) -> BasicInfoV1 {
        let info: BasicInfo = self.collector.collect_basic_info();
        let supplement = self.supplement.basic();
        build_basic_info(&info, &supplement, &self.version)
    }
}
