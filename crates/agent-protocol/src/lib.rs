//! Wire protocol models for the Komari agent.
//!
//! This crate is **pure**: it performs no I/O, opens no sockets and reads no
//! clock. It only encodes and decodes the bytes that [`agent-client`] puts on
//! the wire.
//!
//! * **v1** — plain JSON. [`ReportV1`] is sent to `/api/clients/report`,
//!   [`BasicInfoV1`] to `/api/clients/uploadBasicInfo`. No envelope, no
//!   compression.
//! * **v2** — JSON-RPC 2.0 envelopes around the same v1 payloads, optionally
//!   gzipped for HTTP POST. See [`v2`] and [`compress`].
//!
//! # Field compatibility
//!
//! Field names, types and optionality follow the Go implementation
//! (`komari-agent/monitoring`, `komari-agent/server`,
//! `komari-agent/protocol/v2`) and the panel's decoding structs
//! (`common.Report`, `common.ClientInfo`, `utils/rpc`). Unknown fields are
//! accepted everywhere and preserved in the v1 models, because the Go agent
//! builds those payloads from `map[string]interface{}`.
//!
//! # Numbers
//!
//! Byte sizes, uptime and connection counters are unsigned 64-bit integers on
//! the agent side (`uint64`), while the panel stores the same values in
//! `int64`. Both are encoded as JSON integers. [`serde_json`] keeps integers
//! exact up to `u64::MAX`; no value is routed through `f64`. The one place Go
//! itself loses precision is the v2 report envelope, which decodes the v1 JSON
//! into `interface{}`; see the crate README for that documented difference.
//!
//! [`agent-client`]: https://docs.rs/agent-client

mod compress;
mod error;
pub mod v1;
pub mod v2;

pub use compress::{
    GZIP_DEFAULT_LEVEL, GZIP_OS_UNKNOWN, compress, compress_with_level, decompress,
};
pub use error::ProtocolError;
pub use v1::{
    BasicInfoV1, ConnectionsReportV1, CpuReportV1, DiskReportV1, GpuDetailReportV1,
    GpuDeviceInfoV1, LoadReportV1, NetworkReportV1, RamReportV1, ReportV1,
};
pub use v2::{
    ABORTED, ALREADY_EXISTS, AgentBasicInfoParams, AgentReportParams, CANCELLED, DATA_LOSS,
    DEADLINE_EXCEEDED, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, JSONRPC_VERSION,
    METHOD_AGENT_BASIC_INFO, METHOD_AGENT_REPORT, METHOD_NOT_FOUND, NOT_FOUND, OUT_OF_RANGE,
    PARSE_ERROR, PERMISSION_DENIED, RPC_VERSION, RpcError, RpcRequest, RpcResponse,
    UNAUTHENTICATED, UNAVAILABLE, UNIMPLEMENTED, basic_info_notification, basic_info_params,
    basic_info_request, report_notification, report_params, report_params_from_value,
    report_request,
};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Encode any protocol v1 model as compact JSON.
///
/// Equivalent to Go's `json.Marshal`: no whitespace, no trailing newline, keys
/// in struct declaration order. Use it with [`ReportV1`] or [`BasicInfoV1`]:
///
/// ```
/// use agent_protocol::{encode_v1, ReportV1};
///
/// let report = ReportV1::default();
/// let json = encode_v1(&report).unwrap();
/// assert!(json.starts_with(r#"{"cpu":{},"#));
/// ```
pub fn encode_v1<T: Serialize + ?Sized>(value: &T) -> Result<String, ProtocolError> {
    Ok(serde_json::to_string(value)?)
}

/// Encode any protocol v1 model as UTF-8 JSON bytes.
pub fn encode_v1_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    Ok(serde_json::to_vec(value)?)
}

/// Decode any protocol v1 model from JSON.
///
/// Unknown fields are ignored unless the target model keeps them (the v1
/// models do, via a flattened `extra` map).
pub fn decode_v1<T: DeserializeOwned>(json: &str) -> Result<T, ProtocolError> {
    Ok(serde_json::from_str(json)?)
}

/// Decode any protocol v1 model from UTF-8 JSON bytes.
pub fn decode_v1_slice<T: DeserializeOwned>(json: &[u8]) -> Result<T, ProtocolError> {
    Ok(serde_json::from_slice(json)?)
}
