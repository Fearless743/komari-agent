//! Mapping from collected metrics to protocol v1 payloads.
//!
//! The mapping is pure so it can be tested with synthetic data. Production
//! collection lives in [`crate::monitor`]; this module owns the exact naming
//! and `omitempty` semantics of the Go `monitoring.GenerateReport` payload.

use agent_monitor::{LoadAverage, RamReport, ReportMetrics};
use agent_protocol::{
    ConnectionsReportV1, CpuReportV1, DiskReportV1, GpuDeviceInfoV1, GpuReportV1, LoadReportV1,
    NetworkReportV1, RamReportV1, ReportV1,
};

/// Disk usage section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    pub used: u64,
}

/// Network counters and per-second rates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkSample {
    /// Bytes sent during the last sampling window.
    pub up: u64,
    /// Bytes received during the last sampling window.
    pub down: u64,
    /// Total bytes sent since boot (or since the monthly reset).
    pub total_up: u64,
    /// Total bytes received since boot (or since the monthly reset).
    pub total_down: u64,
}

/// TCP/UDP connection counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionSample {
    pub tcp: i64,
    pub udp: i64,
}

/// Everything needed to build a v1 report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReportSample {
    pub cpu_usage: f64,
    pub ram: RamReport,
    pub swap: RamReport,
    pub load: LoadAverage,
    pub disk: DiskUsage,
    pub network: NetworkSample,
    pub connections: ConnectionSample,
    pub uptime: u64,
    pub process: i64,
    pub gpu: Option<GpuReportV1>,
    pub message: String,
}

impl ReportSample {
    /// Start from the core metrics owned by `agent-monitor`.
    pub fn from_metrics(metrics: ReportMetrics) -> Self {
        Self {
            cpu_usage: metrics.cpu.usage,
            ram: metrics.ram,
            swap: metrics.swap,
            load: metrics.load,
            uptime: metrics.uptime,
            ..Self::default()
        }
    }
}

/// Build the protocol v1 report.
///
/// Go clamps CPU usage to at least `0.001`; the collector already does that,
/// but the mapping repeats it so an injected sample can never produce zero.
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
        gpu: sample.gpu.clone(),
        uptime: sample.uptime,
        process: sample.process,
        message: sample.message.clone(),
        ..ReportV1::default()
    }
}

fn clamped_cpu(usage: f64) -> f64 {
    if usage.is_nan() || usage <= 0.001 {
        0.001
    } else {
        usage
    }
}

/// Detailed GPU shape: `{"count": N, "average_usage": x, "detailed_info": [...]}`.
///
/// Go divides by the number of devices without a zero check, so callers must
/// not pass an empty slice. An empty slice yields `average_usage: 0` here.
pub fn detailed_gpu_report(devices: &[GpuDeviceInfoV1]) -> GpuReportV1 {
    let total_usage: f64 = devices.iter().map(|device| device.utilization).sum();
    let average_usage = if devices.is_empty() {
        0.0
    } else {
        total_usage / devices.len() as f64
    };

    GpuReportV1 {
        count: Some(devices.len() as i64),
        average_usage: Some(average_usage),
        detailed_info: Some(devices.to_vec()),
        ..GpuReportV1::default()
    }
}

/// Model-only fallback used when detailed probing fails.
pub fn gpu_models_report(models: Vec<String>) -> Option<GpuReportV1> {
    if models.is_empty() {
        None
    } else {
        Some(GpuReportV1 {
            models: Some(models),
            ..GpuReportV1::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_sample() -> ReportSample {
        ReportSample {
            cpu_usage: 12.5,
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
            disk: DiskUsage {
                total: 1000,
                used: 400,
            },
            network: NetworkSample {
                up: 10,
                down: 20,
                total_up: 30,
                total_down: 40,
            },
            connections: ConnectionSample { tcp: 7, udp: 3 },
            uptime: 3600,
            process: 42,
            gpu: gpu_models_report(vec!["NVIDIA GeForce RTX 4090".to_owned()]),
            message: "ok".to_owned(),
        }
    }

    #[test]
    fn maps_every_section_with_the_go_field_names() {
        let json = build_report(&full_sample()).to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["cpu"]["usage"], 12.5);
        assert_eq!(value["ram"]["total"], 17_179_869_184u64);
        assert_eq!(value["swap"]["used"], 536_870_912u64);
        assert_eq!(value["load"]["load1"], 1.0);
        assert_eq!(value["load"]["load15"], 0.25);
        assert_eq!(value["disk"]["total"], 1000);
        assert_eq!(value["network"]["totalUp"], 30);
        assert_eq!(value["network"]["totalDown"], 40);
        assert_eq!(value["network"]["up"], 10);
        assert_eq!(value["connections"]["tcp"], 7);
        assert_eq!(value["uptime"], 3600);
        assert_eq!(value["process"], 42);
        assert_eq!(value["gpu"]["models"][0], "NVIDIA GeForce RTX 4090");
        assert_eq!(value["message"], "ok");
        // Go always emits a `cpu` object even on a fully idle machine.
        assert!(value.get("uuid").is_none());
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
    fn detailed_gpu_shape_matches_go() {
        let devices = vec![
            GpuDeviceInfoV1 {
                name: "GPU A".to_owned(),
                memory_total: 100,
                memory_used: 50,
                utilization: 20.0,
                temperature: 40,
                ..GpuDeviceInfoV1::default()
            },
            GpuDeviceInfoV1 {
                name: "GPU B".to_owned(),
                memory_total: 200,
                memory_used: 100,
                utilization: 40.0,
                temperature: 50,
                ..GpuDeviceInfoV1::default()
            },
        ];

        let gpu = detailed_gpu_report(&devices);
        assert_eq!(gpu.count, Some(2));
        assert_eq!(gpu.average_usage, Some(30.0));
        assert_eq!(gpu.detailed_info.as_ref().unwrap().len(), 2);
        assert!(gpu.models.is_none());
    }

    #[test]
    fn gpu_model_fallback_is_none_when_detection_failed() {
        assert!(gpu_models_report(Vec::new()).is_none());

        let mut sample = full_sample();
        sample.gpu = gpu_models_report(Vec::new());
        let value: serde_json::Value =
            serde_json::from_str(&build_report(&sample).to_json().unwrap()).unwrap();
        assert!(value.get("gpu").is_none());
    }

    #[test]
    fn collector_metrics_become_the_core_sample() {
        let metrics = ReportMetrics {
            cpu: agent_monitor::CpuReport { usage: 0.001 },
            ram: RamReport { total: 10, used: 4 },
            swap: RamReport { total: 2, used: 1 },
            load: LoadAverage {
                load1: 1.0,
                load5: 2.0,
                load15: 3.0,
            },
            uptime: 99,
        };

        let sample = ReportSample::from_metrics(metrics);
        assert_eq!(sample.cpu_usage, 0.001);
        assert_eq!(sample.ram.total, 10);
        assert_eq!(sample.swap.used, 1);
        assert_eq!(sample.load.load15, 3.0);
        assert_eq!(sample.uptime, 99);
        // Supplemental sections stay at their zero defaults.
        assert_eq!(sample.disk.total, 0);
        assert_eq!(sample.process, 0);
        assert!(sample.gpu.is_none());
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
