//! Golden tests for the protocol v1 wire models.
//!
//! All golden JSON constants below were produced by the Go implementation
//! (`encoding/json`), not by this crate, so a round trip proves
//! cross-implementation compatibility.

use agent_protocol::v1::{BasicInfoV1, NetworkReportV1, RamReportV1, ReportV1};
use agent_protocol::{ProtocolError, decode_v1, encode_v1};
use serde_json::{Value, json};

/// `monitoring.GenerateReport()` output for fixed metrics, marshalled by Go.
const REPORT_GOLDEN: &str = r#"{"connections":{"tcp":42,"udp":7},"cpu":{"usage":12.3456789},"disk":{"total":1000204886016,"used":500000000000},"load":{"load1":0.5,"load15":0.125,"load5":0.25},"message":"","network":{"down":9876543210987,"totalDown":9223372036854775807,"totalUp":9007199254740993,"up":1234567890123},"process":321,"ram":{"total":34359738368,"used":8589934592},"swap":{"total":0,"used":0},"uptime":123456}"#;

/// `monitoring.GenerateReport()` GPU section when detailed probing succeeds
/// (two devices, second one with `uint64` max memory), marshalled by Go.
const GPU_DETAILED_GOLDEN: &str = r#"{"gpu":{"average_usage":37.5,"count":2,"detailed_info":[{"memory_total":25757220864,"memory_used":1073741824,"name":"NVIDIA GeForce RTX 4090","temperature":61,"utilization":35.25},{"memory_total":18446744073709551615,"memory_used":0,"name":"NVIDIA GeForce RTX 3090","temperature":0,"utilization":12.5}]}}"#;

/// `monitoring.GenerateReport()` GPU fallback when detailed probing fails but
/// model detection succeeds, marshalled by Go.
const GPU_MODELS_GOLDEN: &str =
    r#"{"gpu":{"models":["NVIDIA GeForce RTX 4090","NVIDIA GeForce RTX 3090"]}}"#;

/// `server.uploadBasicInfo` map marshalled by Go.
const BASIC_INFO_GOLDEN: &str = r#"{"arch":"amd64","cpu_cores":16,"cpu_name":"AMD Ryzen 9 7950X 16-Core Processor","cpu_physical_cores":8,"disk_total":1000204886016,"gpu_name":"NVIDIA GeForce RTX 4090","ipv4":"203.0.113.42","ipv6":"","kernel_version":"6.8.0-51-generic","mem_total":34359738368,"os":"Ubuntu 24.04.3 LTS","swap_total":8589934592,"version":"1.2.3","virtualization":"kvm"}"#;

fn as_value(json: &str) -> Value {
    serde_json::from_str(json).expect("golden JSON parses")
}

// ---------------------------------------------------------------------------
// v1 report
// ---------------------------------------------------------------------------

#[test]
fn report_v1_decodes_go_golden() {
    let report = decode_v1::<ReportV1>(REPORT_GOLDEN).expect("decode Go report");

    assert_eq!(report.cpu.usage, 12.3456789);
    assert_eq!(report.cpu.name, "");
    assert_eq!(report.ram.total, 34_359_738_368);
    assert_eq!(report.ram.used, 8_589_934_592);
    assert_eq!(report.swap.total, 0);
    assert_eq!(report.disk.total, 1_000_204_886_016);
    assert_eq!(report.load.load1, 0.5);
    assert_eq!(report.load.load5, 0.25);
    assert_eq!(report.load.load15, 0.125);
    assert_eq!(report.network.up, 1_234_567_890_123);
    assert_eq!(report.network.down, 9_876_543_210_987);
    assert_eq!(report.network.total_up, 9_007_199_254_740_993);
    assert_eq!(report.network.total_down, 9_223_372_036_854_775_807);
    assert_eq!(report.connections.tcp, 42);
    assert_eq!(report.connections.udp, 7);
    assert_eq!(report.uptime, 123_456);
    assert_eq!(report.process, 321);
    assert_eq!(report.message, "");
    assert!(report.uuid.is_none());
    assert!(report.gpu.is_none());
    assert!(report.extra.is_empty());
}

#[test]
fn report_v1_reencodes_to_go_golden_value() {
    let report = decode_v1::<ReportV1>(REPORT_GOLDEN).expect("decode Go report");
    let reencoded = report.to_json().expect("encode report");
    assert_eq!(as_value(&reencoded), as_value(REPORT_GOLDEN));
}

#[test]
fn report_v1_preserves_unknown_fields() {
    let mut value = as_value(REPORT_GOLDEN);
    value["custom_metric"] = json!({"nested": [1, 2, 3]});
    value["agent_flavor"] = json!("third-party");
    let golden = serde_json::to_string(&value).unwrap();

    let report = decode_v1::<ReportV1>(&golden).expect("decode report with extras");
    assert_eq!(report.extra.len(), 2);
    assert_eq!(report.extra["agent_flavor"], json!("third-party"));

    let reencoded = report.to_json().unwrap();
    assert_eq!(as_value(&reencoded), value);
}

#[test]
fn report_v1_omits_go_omitempty_fields() {
    let minimal = r#"{"cpu":{"usage":0.001}}"#;
    let report = decode_v1::<ReportV1>(minimal).expect("minimal report decodes");
    let encoded = report.to_json().unwrap();
    let value = as_value(&encoded);

    assert!(value.get("uuid").is_none());
    assert!(value.get("method").is_none());
    assert!(value.get("updated_at").is_none());
    assert!(value.get("gpu").is_none());
    // Go's `cpu` fields are all omitempty.
    assert_eq!(value["cpu"], json!({"usage": 0.001}));
    // `message` has no omitempty in Go, so it is always present.
    assert_eq!(value["message"], json!(""));
    // Missing sections default to zero and are still serialised.
    assert_eq!(value["ram"], json!({"total": 0, "used": 0}));
    assert_eq!(value["uptime"], json!(0));
}

#[test]
fn report_v1_keeps_int64_and_uint64_exact() {
    let report = ReportV1 {
        network: NetworkReportV1 {
            up: 1,
            down: 2,
            total_up: u64::MAX,
            total_down: i64::MAX as u64,
            ..NetworkReportV1::default()
        },
        uptime: 9_007_199_254_740_993,
        process: i64::MAX,
        ram: RamReportV1 {
            total: u64::MAX,
            used: 1,
            ..RamReportV1::default()
        },
        ..ReportV1::default()
    };
    let encoded = report.to_json().unwrap();

    assert!(encoded.contains("\"totalUp\":18446744073709551615"));
    assert!(encoded.contains("\"totalDown\":9223372036854775807"));
    assert!(encoded.contains("\"uptime\":9007199254740993"));
    assert!(encoded.contains("\"process\":9223372036854775807"));
    assert!(encoded.contains("\"total\":18446744073709551615"));

    let decoded = decode_v1::<ReportV1>(&encoded).unwrap();
    assert_eq!(decoded, report);
    assert_eq!(decoded.to_json().unwrap(), encoded);
}

#[test]
fn report_v1_preserves_float_precision() {
    let golden = r#"{"cpu":{"usage":12.345678901234567},"load":{"load1":0.30000000000000004,"load5":1e-7,"load15":1.7976931348623157e308}}"#;
    let report = decode_v1::<ReportV1>(golden).unwrap();

    assert_eq!(report.cpu.usage, 12.345678901234567_f64);
    assert_eq!(report.load.load1, 0.1_f64 + 0.2_f64);
    assert_eq!(report.load.load5, 1e-7);
    assert_eq!(report.load.load15, f64::MAX);

    let encoded = report.to_json().unwrap();
    assert!(encoded.contains("\"usage\":12.345678901234567"));
    assert!(encoded.contains("\"load1\":0.30000000000000004"));
    assert!(encoded.contains("\"load15\":1.7976931348623157e+308"));
    assert_eq!(decode_v1::<ReportV1>(&encoded).unwrap(), report);
}

#[test]
fn report_v1_decodes_gpu_detailed_go_golden() {
    let report = decode_v1::<ReportV1>(GPU_DETAILED_GOLDEN).unwrap();
    let gpu = report.gpu.as_ref().expect("gpu present");
    assert_eq!(gpu.count, Some(2));
    assert_eq!(gpu.average_usage, Some(37.5));
    assert_eq!(gpu.models, None);
    let devices = gpu.detailed_info.as_ref().expect("detailed info present");
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].name, "NVIDIA GeForce RTX 4090");
    assert_eq!(devices[0].memory_total, 25_757_220_864);
    assert_eq!(devices[0].memory_used, 1_073_741_824);
    assert_eq!(devices[0].temperature, 61);
    // Go agent uses uint64 for memory and temperature.
    assert_eq!(devices[1].memory_total, u64::MAX);
    assert_eq!(devices[1].temperature, 0);

    let reencoded = as_value(&report.to_json().unwrap());
    assert_eq!(reencoded["gpu"], as_value(GPU_DETAILED_GOLDEN)["gpu"]);
}

#[test]
fn report_v1_decodes_gpu_models_fallback_go_golden() {
    let report = decode_v1::<ReportV1>(GPU_MODELS_GOLDEN).unwrap();
    let gpu = report.gpu.as_ref().expect("gpu present");
    assert_eq!(
        gpu.models,
        Some(vec![
            "NVIDIA GeForce RTX 4090".to_string(),
            "NVIDIA GeForce RTX 3090".to_string(),
        ])
    );
    assert_eq!(gpu.count, None);
    assert_eq!(gpu.average_usage, None);
    assert_eq!(gpu.detailed_info, None);

    let reencoded = report.to_json().unwrap();
    assert_eq!(
        as_value(&reencoded)["gpu"],
        as_value(GPU_MODELS_GOLDEN)["gpu"]
    );
    // The fallback shape must not be padded with zero-valued detailed fields.
    assert!(!reencoded.contains("count"));
    assert!(!reencoded.contains("average_usage"));
    assert!(!reencoded.contains("detailed_info"));
}

#[test]
fn report_v1_preserves_nested_unknown_fields() {
    let golden = r#"{"cpu":{"usage":1.5,"vendor":"AuthenticAMD","flags":["sse4_2"]},"ram":{"total":123,"used":1,"hugepages":2},"swap":{"total":0,"used":0,"zram":true},"load":{"load1":0.1,"load5":0.2,"load15":0.3,"runnable":5},"disk":{"total":10,"used":5,"inodes":100},"network":{"up":1,"down":2,"totalUp":3,"totalDown":4,"iface":"eth0"},"connections":{"tcp":1,"udp":2,"unix":3},"gpu":{"count":1,"average_usage":37.5,"detailed_info":[{"name":"gpu","memory_total":1,"memory_used":0,"utilization":35.5,"temperature":0,"pci":"0000:01:00.0"}],"driver":"nvidia"},"custom_section":{"x":1},"uptime":0,"process":0,"message":""}"#;
    let report = decode_v1::<ReportV1>(golden).unwrap();

    assert_eq!(report.cpu.extra["vendor"], json!("AuthenticAMD"));
    assert_eq!(report.ram.extra["hugepages"], json!(2));
    assert_eq!(report.swap.extra["zram"], json!(true));
    assert_eq!(report.load.extra["runnable"], json!(5));
    assert_eq!(report.disk.extra["inodes"], json!(100));
    assert_eq!(report.network.extra["iface"], json!("eth0"));
    assert_eq!(report.connections.extra["unix"], json!(3));
    assert_eq!(report.extra["custom_section"], json!({"x": 1}));
    let gpu = report.gpu.as_ref().unwrap();
    assert_eq!(gpu.extra["driver"], json!("nvidia"));
    assert_eq!(
        gpu.detailed_info.as_ref().unwrap()[0].extra["pci"],
        json!("0000:01:00.0")
    );

    let reencoded = report.to_json().unwrap();
    assert_eq!(as_value(&reencoded), as_value(golden));
}

#[test]
fn report_v1_treats_null_sections_as_zero_values() {
    let golden = r#"{"cpu":null,"ram":null,"swap":null,"load":null,"disk":null,"network":null,"connections":null,"gpu":null,"uptime":null,"process":null,"message":null,"uuid":null,"method":null,"updated_at":null}"#;
    let report = decode_v1::<ReportV1>(golden).expect("Go accepts null for every field");

    assert_eq!(report.cpu, Default::default());
    assert_eq!(report.ram, Default::default());
    assert_eq!(report.swap, Default::default());
    assert_eq!(report.load, Default::default());
    assert_eq!(report.disk, Default::default());
    assert_eq!(report.network, Default::default());
    assert_eq!(report.connections, Default::default());
    assert_eq!(report.gpu, None);
    assert_eq!(report.uptime, 0);
    assert_eq!(report.process, 0);
    assert_eq!(report.message, "");
    assert!(report.uuid.is_none());
    assert!(report.method.is_none());
    assert!(report.updated_at.is_none());

    assert_eq!(
        report.to_json().unwrap(),
        encode_v1(&ReportV1::default()).unwrap()
    );
}

#[test]
fn report_v1_treats_nested_null_scalars_as_zero() {
    let golden = r#"{"cpu":{"name":null,"cores":null,"arch":null,"usage":null},"ram":{"total":null,"used":null},"swap":{"total":null,"used":null},"load":{"load1":null,"load5":null,"load15":null},"disk":{"total":null,"used":null},"network":{"up":null,"down":null,"totalUp":null,"totalDown":null},"connections":{"tcp":null,"udp":null},"uptime":null,"process":null,"message":null}"#;
    let report = decode_v1::<ReportV1>(golden).expect("null scalars become zero");

    assert_eq!(report.cpu, Default::default());
    assert_eq!(report.ram, Default::default());
    assert_eq!(report.swap, Default::default());
    assert_eq!(report.load, Default::default());
    assert_eq!(report.disk, Default::default());
    assert_eq!(report.network, Default::default());
    assert_eq!(report.connections, Default::default());
    assert_eq!(report.message, "");
    assert_eq!(
        report.to_json().unwrap(),
        encode_v1(&ReportV1::default()).unwrap()
    );
}

// ---------------------------------------------------------------------------
// v1 basic info
// ---------------------------------------------------------------------------

#[test]
fn basic_info_v1_decodes_and_reencodes_go_golden() {
    let info = decode_v1::<BasicInfoV1>(BASIC_INFO_GOLDEN).expect("decode Go basic info");
    assert_eq!(info.cpu_cores, 16);
    assert_eq!(info.cpu_physical_cores, Some(8));
    assert_eq!(info.kernel_version.as_deref(), Some("6.8.0-51-generic"));
    assert_eq!(info.ipv6, "", "Go keeps the empty string");
    assert_eq!(info.mem_total, 34_359_738_368);
    assert_eq!(info.swap_total, 8_589_934_592);
    assert_eq!(info.disk_total, 1_000_204_886_016);
    assert_eq!(info.virtualization, "kvm");
    assert!(info.uuid.is_none());
    assert!(info.region.is_none());

    let reencoded = info.to_json().unwrap();
    assert_eq!(as_value(&reencoded), as_value(BASIC_INFO_GOLDEN));
}

#[test]
fn basic_info_v1_preserves_unknown_fields() {
    let mut value = as_value(BASIC_INFO_GOLDEN);
    value["platform"] = json!("third-party-panel");
    value["labels"] = json!({"env": "prod"});
    let golden = serde_json::to_string(&value).unwrap();

    let info = decode_v1::<BasicInfoV1>(&golden).unwrap();
    assert_eq!(info.extra.len(), 2);
    let reencoded = info.to_json().unwrap();
    assert_eq!(as_value(&reencoded), value);
}

#[test]
fn basic_info_v1_can_drop_legacy_keys() {
    // The constructed default mirrors Go's full upload: both legacy keys are
    // present (with zero/empty values) until the panel rejects them.
    let full_default = BasicInfoV1::default().to_json().unwrap();
    assert!(full_default.contains(r#""cpu_physical_cores":0"#));
    assert!(full_default.contains(r#""kernel_version":"""#));

    let mut info = decode_v1::<BasicInfoV1>(BASIC_INFO_GOLDEN).unwrap();
    // Go deletes these keys for panels <= 1.0.2 / <= 1.2.0.
    info.kernel_version = None;
    info.cpu_physical_cores = None;

    let encoded = info.to_json().unwrap();
    assert!(!encoded.contains("kernel_version"));
    assert!(!encoded.contains("cpu_physical_cores"));
    let decoded = decode_v1::<BasicInfoV1>(&encoded).unwrap();
    assert_eq!(decoded, info);
}

#[test]
fn basic_info_v1_missing_fields_default_like_go() {
    let info = decode_v1::<BasicInfoV1>(r#"{"cpu_name":"Unknown"}"#).unwrap();
    assert_eq!(info.cpu_name, "Unknown");
    assert_eq!(info.cpu_cores, 0);
    assert_eq!(info.ipv6, "");
    assert_eq!(info.mem_total, 0);
    assert!(info.extra.is_empty());
}

// ---------------------------------------------------------------------------
// generic helpers
// ---------------------------------------------------------------------------

#[test]
fn encode_decode_helpers_report_errors() {
    assert!(matches!(
        decode_v1::<ReportV1>("not json"),
        Err(ProtocolError::Json(_))
    ));
    assert!(matches!(
        decode_v1::<ReportV1>(r#"{"ram":"not an object"}"#),
        Err(ProtocolError::Json(_))
    ));
    assert!(matches!(
        decode_v1::<ReportV1>(r#"{"cpu":{}} trailing"#),
        Err(ProtocolError::Json(_))
    ));

    let encoded = encode_v1(&ReportV1::default()).unwrap();
    let decoded = decode_v1::<ReportV1>(&encoded).unwrap();
    assert_eq!(decoded, ReportV1::default());
}

#[test]
fn empty_report_serialises_like_go_zero_value() {
    let encoded = encode_v1(&ReportV1::default()).unwrap();
    assert_eq!(
        encoded,
        r#"{"cpu":{},"ram":{"total":0,"used":0},"swap":{"total":0,"used":0},"load":{"load1":0.0,"load5":0.0,"load15":0.0},"disk":{"total":0,"used":0},"network":{"up":0,"down":0,"totalUp":0,"totalDown":0},"connections":{"tcp":0,"udp":0},"uptime":0,"process":0,"message":""}"#
    );
    // The zero value round trips through the typed model.
    assert_eq!(
        decode_v1::<ReportV1>(&encoded).unwrap(),
        ReportV1::default()
    );
}
