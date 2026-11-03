//! Mapping from `agent-monitor` metrics to protocol v1 payloads.
//!
//! The mapping is pure so it can be tested with synthetic data. A03 owns the
//! collection (disk, network, connections, process, GPU, message); this module
//! only renames fields and applies Go's CPU floor.

use agent_monitor::{
    ConnectionsReport, DiskInfo, GpuReport, LoadAverage, NetworkReport, RamReport, ReportMetrics,
};
use agent_protocol::{
    ConnectionsReportV1, CpuReportV1, DiskReportV1, GpuDeviceInfoV1, GpuReportV1, LoadReportV1,
    NetworkReportV1, RamReportV1, ReportV1,
};

/// Everything needed to build a v1 report.
///
/// This mirrors [`ReportMetrics`] field for field on purpose: adding a section
/// in `agent-monitor` must be a compile error here, never a silently dropped
/// value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReportSample {
    pub cpu_usage: f64,
    pub ram: RamReport,
    pub swap: RamReport,
    pub load: LoadAverage,
    pub disk: DiskInfo,
    pub network: NetworkReport,
    pub connections: ConnectionsReport,
    pub gpu: Option<GpuReport>,
    pub uptime: u64,
    pub process: i64,
    pub message: String,
}

impl ReportSample {
    /// Convert A03 metrics into a report sample without defaulting any field.
    pub fn from_metrics(metrics: ReportMetrics) -> Self {
        Self {
            cpu_usage: metrics.cpu.usage,
            ram: metrics.ram,
            swap: metrics.swap,
            load: metrics.load,
            disk: metrics.disk,
            network: metrics.network,
            connections: metrics.connections,
            gpu: metrics.gpu,
            uptime: metrics.uptime,
            process: metrics.process,
            message: metrics.message,
        }
    }
}

/// Build the protocol v1 report.
///
/// Go clamps CPU usage to at least `0.001`; A03 already does that, but the
/// mapping repeats it so an injected sample can never produce zero.
pub fn build_report(sample: &ReportSample) -> ReportV1 {
    ReportV1 {
        cpu: CpuReportV1 {
            usage: clamped_cpu(sample.cpu_usage),
            ..CpuReportV1::default()
        },
        ram: RamReportV1 {
            total: sample.ram.total,
            used: sample.ram.used,
            ..RamReportV1::default()
        },
        swap: RamReportV1 {
            total: sample.swap.total,
            used: sample.swap.used,
            ..RamReportV1::default()
        },
        load: LoadReportV1 {
            load1: sample.load.load1,
            load5: sample.load.load5,
            load15: sample.load.load15,
            ..LoadReportV1::default()
        },
        disk: DiskReportV1 {
            total: sample.disk.total,
            used: sample.disk.used,
            ..DiskReportV1::default()
        },
        network: NetworkReportV1 {
            up: sample.network.up,
            down: sample.network.down,
            total_up: sample.network.total_up,
            total_down: sample.network.total_down,
            ..NetworkReportV1::default()
        },
        connections: ConnectionsReportV1 {
            tcp: sample.connections.tcp,
            udp: sample.connections.udp,
            ..ConnectionsReportV1::default()
        },
        gpu: sample.gpu.as_ref().map(gpu_to_v1),
        uptime: sample.uptime,
        process: sample.process,
        message: sample.message.clone(),
        ..ReportV1::default()
    }
}

/// Convert A03's GPU report (detailed or model-only) to the wire type.
fn gpu_to_v1(gpu: &GpuReport) -> GpuReportV1 {
    GpuReportV1 {
        count: gpu.count,
        average_usage: gpu.average_usage,
        detailed_info: gpu.detailed_info.as_ref().map(|devices| {
            devices
                .iter()
                .map(|device| GpuDeviceInfoV1 {
                    name: device.name.clone(),
                    memory_total: device.memory_total,
                    memory_used: device.memory_used,
                    utilization: device.utilization,
                    temperature: device.temperature,
                    ..GpuDeviceInfoV1::default()
                })
                .collect()
        }),
        models: gpu.models.clone(),
        ..GpuReportV1::default()
    }
}

fn clamped_cpu(usage: f64) -> f64 {
    if usage.is_nan() || usage <= 0.001 {
        0.001
    } else {
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_monitor::{CpuReport, GpuInfo, ReportMetrics};

    fn full_metrics() -> ReportMetrics {
        ReportMetrics {
            cpu: CpuReport { usage: 12.5 },
            ram: RamReport {
                total: 16 * 1024 * 1024 * 1024,
                used: 4 * 1024 * 1024 * 1024,
            },
            swap: RamReport {
                total: 2 * 1024 * 1024 * 1024,
                used: 512 * 1024 * 1024,
            },
            load: LoadAverage {
                load1: 1.0,
                load5: 0.5,
                load15: 0.25,
            },
            disk: DiskInfo {
                total: 1000,
                used: 400,
            },
            network: NetworkReport {
                up: 10,
                down: 20,
                total_up: 30,
                total_down: 40,
            },
            connections: ConnectionsReport { tcp: 7, udp: 3 },
            gpu: GpuReport::models(vec!["NVIDIA GeForce RTX 4090".to_owned()]),
            uptime: 3600,
            process: 42,
            message: "ok".to_owned(),
        }
    }

    #[test]
    fn from_metrics_keeps_every_a03_field() {
        let metrics = full_metrics();
        let sample = ReportSample::from_metrics(metrics.clone());

        assert_eq!(sample.cpu_usage, metrics.cpu.usage);
        assert_eq!(sample.ram, metrics.ram);
        assert_eq!(sample.swap, metrics.swap);
        assert_eq!(sample.load, metrics.load);
        assert_eq!(sample.disk, metrics.disk);
        assert_eq!(sample.network, metrics.network);
        assert_eq!(sample.connections, metrics.connections);
        assert_eq!(sample.gpu, metrics.gpu);
        assert_eq!(sample.uptime, metrics.uptime);
        assert_eq!(sample.process, metrics.process);
        assert_eq!(sample.message, metrics.message);
    }

    #[test]
    fn maps_every_section_with_the_go_field_names() {
        let json = build_report(&ReportSample::from_metrics(full_metrics()))
            .to_json()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["cpu"]["usage"], 12.5);
        assert_eq!(value["ram"]["total"], 17_179_869_184u64);
        assert_eq!(value["swap"]["used"], 536_870_912u64);
        assert_eq!(value["load"]["load1"], 1.0);
        assert_eq!(value["load"]["load15"], 0.25);
        assert_eq!(value["disk"]["total"], 1000);
        assert_eq!(value["disk"]["used"], 400);
        assert_eq!(value["network"]["totalUp"], 30);
        assert_eq!(value["network"]["totalDown"], 40);
        assert_eq!(value["network"]["up"], 10);
        assert_eq!(value["network"]["down"], 20);
        assert_eq!(value["connections"]["tcp"], 7);
        assert_eq!(value["connections"]["udp"], 3);
        assert_eq!(value["uptime"], 3600);
        assert_eq!(value["process"], 42);
        assert_eq!(value["gpu"]["models"][0], "NVIDIA GeForce RTX 4090");
        assert_eq!(value["message"], "ok");
        assert!(value.get("uuid").is_none());
    }

    #[test]
    fn detailed_gpu_fields_are_preserved() {
        let mut metrics = full_metrics();
        metrics.gpu = GpuReport::detailed(vec![
            GpuInfo {
                name: "GPU A".to_owned(),
                memory_total: 100,
                memory_used: 50,
                utilization: 20.0,
                temperature: 40,
            },
            GpuInfo {
                name: "GPU B".to_owned(),
                memory_total: 200,
                memory_used: 100,
                utilization: 40.0,
                temperature: 50,
            },
        ]);

        let value: serde_json::Value = serde_json::from_str(
            &build_report(&ReportSample::from_metrics(metrics))
                .to_json()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["gpu"]["count"], 2);
        assert_eq!(value["gpu"]["average_usage"], 30.0);
        assert_eq!(value["gpu"]["detailed_info"][0]["name"], "GPU A");
        assert_eq!(value["gpu"]["detailed_info"][1]["memory_used"], 100);
    }

    #[test]
    fn cpu_usage_has_the_0_001_floor() {
        for usage in [0.0, 0.0001, -5.0, f64::NAN] {
            let sample = ReportSample {
                cpu_usage: usage,
                ..ReportSample::default()
            };
            assert_eq!(build_report(&sample).cpu.usage, 0.001);
        }

        let sample = ReportSample {
            cpu_usage: 0.002,
            ..ReportSample::default()
        };
        assert_eq!(build_report(&sample).cpu.usage, 0.002);
    }

    #[test]
    fn zero_sections_are_still_present() {
        let report = build_report(&ReportSample::default());
        let value: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();

        for key in ["ram", "swap", "load", "disk", "network", "connections"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert_eq!(value["cpu"]["usage"], 0.001);
        assert_eq!(value["process"], 0);
        assert_eq!(value["message"], "");
    }
}
