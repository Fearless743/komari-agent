//! Golden and interoperability tests for the wire protocol.
//!
//! All golden JSON/hex constants below were produced by the Go implementation
//! (`encoding/json`, `compress/gzip` at default level) — they are not generated
//! by this crate, so a round trip proves cross-implementation compatibility.

use agent_protocol::v1::{BasicInfoV1, NetworkReportV1, RamReportV1, ReportV1};
use agent_protocol::v2::{
    self, INVALID_REQUEST, JSONRPC_VERSION, METHOD_AGENT_BASIC_INFO, METHOD_AGENT_REPORT,
    PARSE_ERROR, RpcError, RpcRequest, RpcResponse,
};
use agent_protocol::{
    ProtocolError, compress, compress_with_level, decode_v1, decompress, encode_v1,
    report_notification, report_request,
};
use serde_json::{Value, json};

/// `monitoring.GenerateReport()` output for fixed metrics, marshalled by Go.
const REPORT_GOLDEN: &str = r#"{"connections":{"tcp":42,"udp":7},"cpu":{"usage":12.3456789},"disk":{"total":1000204886016,"used":500000000000},"load":{"load1":0.5,"load15":0.125,"load5":0.25},"message":"","network":{"down":9876543210987,"totalDown":9223372036854775807,"totalUp":9007199254740993,"up":1234567890123},"process":321,"ram":{"total":34359738368,"used":8589934592},"swap":{"total":0,"used":0},"uptime":123456}"#;

/// Same report with every number exactly representable as `float64`, so the Go
/// v2 envelope (which round-trips through `interface{}`) does not round.
const REPORT_SMALL_GOLDEN: &str = r#"{"connections":{"tcp":42,"udp":7},"cpu":{"usage":12.5},"disk":{"total":1000204886016,"used":500000000000},"load":{"load1":0.5,"load15":0.125,"load5":0.25},"message":"","network":{"down":9876543210987,"totalDown":123456789,"totalUp":9007199254740992,"up":1234567890123},"process":321,"ram":{"total":34359738368,"used":8589934592},"swap":{"total":0,"used":0},"uptime":123456}"#;

/// `server.uploadBasicInfo` map marshalled by Go.
const BASIC_INFO_GOLDEN: &str = r#"{"arch":"amd64","cpu_cores":16,"cpu_name":"AMD Ryzen 9 7950X 16-Core Processor","cpu_physical_cores":8,"disk_total":1000204886016,"gpu_name":"NVIDIA GeForce RTX 4090","ipv4":"203.0.113.42","ipv6":"","kernel_version":"6.8.0-51-generic","mem_total":34359738368,"os":"Ubuntu 24.04.3 LTS","swap_total":8589934592,"version":"1.2.3","virtualization":"kvm"}"#;

/// `v2.BuildReportPayload` output for [`REPORT_SMALL_GOLDEN`].
const REPORT_NOTIFICATION_GOLDEN: &str = r#"{"jsonrpc":"2.0","method":"agent.report","params":{"report":{"connections":{"tcp":42,"udp":7},"cpu":{"usage":12.5},"disk":{"total":1000204886016,"used":500000000000},"load":{"load1":0.5,"load15":0.125,"load5":0.25},"message":"","network":{"down":9876543210987,"totalDown":123456789,"totalUp":9007199254740992,"up":1234567890123},"process":321,"ram":{"total":34359738368,"used":8589934592},"swap":{"total":0,"used":0},"uptime":123456}}}"#;

/// `v2.BuildReportRequest` output with the POST fallback id format.
const REPORT_REQUEST_GOLDEN: &str = r#"{"jsonrpc":"2.0","method":"agent.report","params":{"report":{"connections":{"tcp":42,"udp":7},"cpu":{"usage":12.5},"disk":{"total":1000204886016,"used":500000000000},"load":{"load1":0.5,"load15":0.125,"load5":0.25},"message":"","network":{"down":9876543210987,"totalDown":123456789,"totalUp":9007199254740992,"up":1234567890123},"process":321,"ram":{"total":34359738368,"used":8589934592},"swap":{"total":0,"used":0},"uptime":123456}},"id":"report-1700000000000000000"}"#;

/// `v2.BuildBasicInfoPayload` output.
const BASIC_INFO_NOTIFICATION_GOLDEN: &str = r#"{"jsonrpc":"2.0","method":"agent.basicInfo","params":{"info":{"arch":"amd64","cpu_cores":16,"cpu_name":"AMD Ryzen 9 7950X 16-Core Processor","cpu_physical_cores":8,"disk_total":1000204886016,"gpu_name":"NVIDIA GeForce RTX 4090","ipv4":"203.0.113.42","ipv6":"","kernel_version":"6.8.0-51-generic","mem_total":34359738368,"os":"Ubuntu 24.04.3 LTS","swap_total":8589934592,"version":"1.2.3","virtualization":"kvm"}}}"#;

/// Error response marshalled by Go's `utils/rpc`.
const RESPONSE_ERROR_GOLDEN: &str = r#"{"error":{"code":-32602,"data":"report required","message":"invalid params"},"id":"report-1","jsonrpc":"2.0"}"#;

/// Success response marshalled by Go's `utils/rpc`.
const RESPONSE_SUCCESS_GOLDEN: &str = r#"{"id":17,"jsonrpc":"2.0","result":{"status":"success"}}"#;

/// `transport.GzipBytes(REPORT_GOLDEN)` (Go default level).
const GZIP_REPORT_HEX: &str = "1f8b08000000000000ff4c8f4d8a03211085eff2d612ca2affd773853940d32d4398a495d84d16c1bb0fa613266ee4f995bcfa1e98cbbae6793b97b5213db0cd15c9b0c2be5424df15e6ba0fb0b7e92723693e89b1ce87d8159673fb1d6c2bdb7441d244c4644270a49dc2def28264e9ff74854b9996f165dc1a894ef678d37604cdaff84c6cbbc235b7a3195058f3762fb767e752ee2b520cde5923ac2906af8e45be0ec22ce299c4056bbcb781defcbb224522af63646bbca1184561af43eee5469aa52bd45b99736b48c25ae1365d3f64c5888d5e82b8f0560d36c428c646ee0aed3ed58f717a0f511f5ddbf99a91348bb1aeff0d00f4e1f25185010000";

/// `transport.GzipBytes([]byte{})`.
const GZIP_EMPTY_HEX: &str = "1f8b08000000000000ff03000000000000000000";

/// `transport.GzipBytes([]byte("hello komari"))`.
const GZIP_HELLO_HEX: &str =
    "1f8b08000000000000ff000c00f3ff68656c6c6f206b6f6d617269030062a89d130c000000";

fn unhex(hex: &str) -> Vec<u8> {
    assert!(hex.len().is_multiple_of(2), "odd hex length");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

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
        },
        uptime: 9_007_199_254_740_993,
        process: i64::MAX,
        ram: RamReportV1 {
            total: u64::MAX,
            used: 1,
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
fn report_v1_decodes_gpu_section() {
    let golden = r#"{"gpu":{"count":2,"average_usage":37.5,"detailed_info":[{"name":"RTX 4090","memory_total":25757220864,"memory_used":1073741824,"utilization":35.0,"temperature":61},{"name":"RTX 3090","memory_total":25757220864,"memory_used":0,"utilization":40.0,"temperature":0}]}}"#;
    let report = decode_v1::<ReportV1>(golden).unwrap();
    let gpu = report.gpu.as_ref().expect("gpu present");
    assert_eq!(gpu.count, 2);
    assert_eq!(gpu.average_usage, 37.5);
    assert_eq!(gpu.detailed_info.len(), 2);
    assert_eq!(gpu.detailed_info[0].name, "RTX 4090");
    assert_eq!(gpu.detailed_info[0].memory_total, 25_757_220_864);
    assert_eq!(gpu.detailed_info[1].temperature, 0);

    // The GPU section is rebuilt from typed fields.
    let round_trip = decode_v1::<ReportV1>(&report.to_json().unwrap()).unwrap();
    assert_eq!(round_trip.gpu, report.gpu);
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
// v2 JSON-RPC
// ---------------------------------------------------------------------------

#[test]
fn v2_report_notification_matches_go_golden() {
    let report = decode_v1::<ReportV1>(REPORT_SMALL_GOLDEN).unwrap();
    let payload = report_notification(&report).unwrap();
    assert_eq!(as_value(&payload), as_value(REPORT_NOTIFICATION_GOLDEN));

    let request = RpcRequest::from_json(&payload).unwrap();
    assert_eq!(request.jsonrpc, JSONRPC_VERSION);
    assert_eq!(request.method, METHOD_AGENT_REPORT);
    assert!(request.is_notification());
    assert_eq!(request.validate(), Ok(()));
}

#[test]
fn v2_report_request_matches_go_golden() {
    let report = decode_v1::<ReportV1>(REPORT_SMALL_GOLDEN).unwrap();
    let payload = report_request("report-1700000000000000000", &report).unwrap();
    assert_eq!(as_value(&payload), as_value(REPORT_REQUEST_GOLDEN));

    let request = RpcRequest::from_json(&payload).unwrap();
    assert!(!request.is_notification());
    assert_eq!(request.id, Some(json!("report-1700000000000000000")));
}

#[test]
fn v2_basic_info_notification_matches_go_golden() {
    let info = decode_v1::<BasicInfoV1>(BASIC_INFO_GOLDEN).unwrap();
    let payload = v2::basic_info_notification(&info).unwrap();
    assert_eq!(as_value(&payload), as_value(BASIC_INFO_NOTIFICATION_GOLDEN));

    let request = RpcRequest::from_json(&payload).unwrap();
    assert_eq!(request.method, METHOD_AGENT_BASIC_INFO);
    assert!(request.is_notification());
    assert!(!payload.contains("\"id\""));
}

#[test]
fn v2_error_response_parses_go_golden() {
    let response = RpcResponse::from_json(RESPONSE_ERROR_GOLDEN).unwrap();
    assert_eq!(response.jsonrpc, JSONRPC_VERSION);
    assert_eq!(response.id, Some(json!("report-1")));
    assert!(response.result.is_none());
    let error = response.error.as_ref().expect("error present");
    assert_eq!(error.code, -32602);
    assert_eq!(error.message, "invalid params");
    assert_eq!(error.data, Some(json!("report required")));

    // Re-encoding keeps the same JSON value (field order is irrelevant).
    assert_eq!(
        as_value(&response.to_json().unwrap()),
        as_value(RESPONSE_ERROR_GOLDEN)
    );
}

#[test]
fn v2_success_response_parses_go_golden() {
    let response = RpcResponse::from_json(RESPONSE_SUCCESS_GOLDEN).unwrap();
    assert_eq!(response.id.as_ref().and_then(Value::as_i64), Some(17));
    assert_eq!(response.result, Some(json!({"status": "success"})));
    assert!(response.error.is_none());
    assert_eq!(
        as_value(&response.to_json().unwrap()),
        as_value(RESPONSE_SUCCESS_GOLDEN)
    );

    let constructed = RpcResponse::success(Some(json!(17)), json!({"status": "success"}));
    assert_eq!(constructed.to_json().unwrap(), response.to_json().unwrap());
}

#[test]
fn v2_request_validation_matches_go() {
    let mut request = RpcRequest::notification(METHOD_AGENT_REPORT, None);
    assert_eq!(request.validate(), Ok(()));

    request.jsonrpc = "1.0".to_string();
    let error = request.validate().unwrap_err();
    assert_eq!(error.code, INVALID_REQUEST);
    assert_eq!(error.message, "invalid jsonrpc version");

    request.jsonrpc = JSONRPC_VERSION.to_string();
    request.method = "   ".to_string();
    assert_eq!(request.validate().unwrap_err().message, "method required");
}

#[test]
fn v2_ignores_unknown_members() {
    let golden = r#"{"jsonrpc":"2.0","method":"agent.report","params":{"report":{}},"id":7,"unknown":{"a":1}}"#;
    let request = RpcRequest::from_json(golden).unwrap();
    assert_eq!(request.id, Some(json!(7)));
    let reencoded = request.to_json().unwrap();
    assert!(!reencoded.contains("unknown"));
}

#[test]
fn v2_error_codes_match_go_constants() {
    let expected = [
        (v2::PARSE_ERROR, -32700),
        (v2::INVALID_REQUEST, -32600),
        (v2::METHOD_NOT_FOUND, -32601),
        (v2::INVALID_PARAMS, -32602),
        (v2::INTERNAL_ERROR, -32603),
        (v2::UNAUTHENTICATED, -32040),
        (v2::PERMISSION_DENIED, -32041),
        (v2::NOT_FOUND, -32044),
        (v2::ALREADY_EXISTS, -32045),
        (v2::CANCELLED, -32010),
        (v2::DEADLINE_EXCEEDED, -32011),
        (v2::ABORTED, -32021),
        (v2::OUT_OF_RANGE, -32022),
        (v2::UNIMPLEMENTED, -32050),
        (v2::UNAVAILABLE, -32051),
        (v2::DATA_LOSS, -32052),
    ];
    for (actual, want) in expected {
        assert_eq!(actual, want);
    }
    assert_eq!(v2::RPC_VERSION, "2.0");

    let error = RpcError::new(PARSE_ERROR, "invalid json").with_data("unexpected EOF");
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({"code": -32700, "message": "invalid json", "data": "unexpected EOF"})
    );
}

// ---------------------------------------------------------------------------
// gzip
// ---------------------------------------------------------------------------

#[test]
fn gzip_round_trip_all_sizes() {
    let payloads: [&[u8]; 4] = [
        b"",
        b"hello komari",
        REPORT_GOLDEN.as_bytes(),
        &[0x5a; 64 * 1024],
    ];
    for payload in payloads {
        let compressed = compress(payload).expect("compress");
        assert!(!compressed.is_empty());
        assert_eq!(&compressed[..2], &[0x1f, 0x8b], "gzip magic");
        assert_eq!(decompress(&compressed).expect("decompress"), payload);
    }
}

#[test]
fn gzip_header_matches_go() {
    // Go writes mtime=0, XFL=0 and OS=255; the first ten bytes are identical
    // for every default-level stream.
    let compressed = compress(b"hello komari").unwrap();
    let go_empty = unhex(GZIP_EMPTY_HEX);
    assert_eq!(&compressed[..10], &go_empty[..10]);

    // Empty input happens to produce byte-identical streams.
    assert_eq!(compress(b"").unwrap(), go_empty);
}

#[test]
fn gzip_decompresses_go_fixed_bytes() {
    assert_eq!(decompress(&unhex(GZIP_HELLO_HEX)).unwrap(), b"hello komari");
    assert_eq!(
        decompress(&unhex(GZIP_REPORT_HEX)).unwrap(),
        REPORT_GOLDEN.as_bytes()
    );
    // Go's gzip.Reader reads concatenated members (multistream); so do we.
    let mut multi = unhex(GZIP_HELLO_HEX);
    multi.extend_from_slice(&unhex(GZIP_EMPTY_HEX));
    assert_eq!(decompress(&multi).unwrap(), b"hello komari");
}

#[test]
fn gzip_rejects_empty_and_corrupt_input() {
    assert!(matches!(decompress(b""), Err(ProtocolError::EmptyInput)));

    let go = unhex(GZIP_HELLO_HEX);

    let bad_magic = {
        let mut data = go.clone();
        data[0] = 0x00;
        data
    };
    assert!(matches!(
        decompress(&bad_magic),
        Err(ProtocolError::Decompress(_))
    ));

    let truncated_header = &go[..5];
    assert!(matches!(
        decompress(truncated_header),
        Err(ProtocolError::Decompress(_))
    ));

    let truncated_body = &go[..go.len() - 4];
    assert!(matches!(
        decompress(truncated_body),
        Err(ProtocolError::Decompress(_))
    ));

    let bad_crc = {
        let mut data = go;
        let last = data.len() - 5;
        data[last] ^= 0xff;
        data
    };
    assert!(matches!(
        decompress(&bad_crc),
        Err(ProtocolError::Decompress(_))
    ));

    assert!(matches!(
        compress_with_level(b"x", 10),
        Err(ProtocolError::InvalidLevel(10))
    ));
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

    let bytes = agent_protocol::encode_v1_vec(&ReportV1::default()).unwrap();
    let decoded = agent_protocol::decode_v1_slice::<ReportV1>(&bytes).unwrap();
    assert_eq!(decoded, ReportV1::default());
    assert_eq!(
        encode_v1(&ReportV1::default()).unwrap(),
        String::from_utf8(bytes).unwrap()
    );
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
