//! Metric provider abstraction.
//!
//! A03 owns all periodic/basic metrics. The provider only adds the fixed
//! one-second CPU window Go uses for every report and the IP-only collector.

use std::sync::Arc;
use std::time::Duration;

use agent_monitor::{BasicInfo, Collector};
use agent_protocol::BasicInfoV1;

use crate::basic_info::build_basic_info;
use crate::ip::{self, IpOptions};
use crate::report::ReportSample;

/// Source of report and basic-info data.
pub trait ReportProvider: Send + Sync {
    fn collect_report(&self) -> ReportSample;
    fn collect_basic_info(&self) -> BasicInfoV1;
}

/// Production provider backed by `agent-monitor` plus the IP-only module.
pub struct MonitorReportProvider {
    collector: Arc<Collector>,
    ip_options: IpOptions,
    cpu_window: Duration,
    version: String,
}

impl MonitorReportProvider {
    pub fn new(
        collector: Arc<Collector>,
        ip_options: IpOptions,
        cpu_window: Duration,
        version: impl Into<String>,
    ) -> Self {
        Self {
            collector,
            ip_options,
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
        // Go's `Cpu()` always samples a one-second window. A03's
        // `collect_report_metrics()` uses its cached CPU sample, so take the
        // fixed window first and override the CPU section with that value.
        let cpu = self.collector.sample_cpu(self.cpu_window);
        let mut metrics = self.collector.collect_report_metrics();
        metrics.cpu.usage = if cpu.usage <= 0.001 { 0.001 } else { cpu.usage };

        ReportSample::from_metrics(metrics)
    }

    fn collect_basic_info(&self) -> BasicInfoV1 {
        let info: BasicInfo = self.collector.collect_basic_info();
        let (ipv4, ipv6) = ip::addresses(&self.ip_options);
        build_basic_info(&info, &ipv4, &ipv6, &self.version)
    }
}
