//! Wire protocol models for the Komari agent.
//!
//! This crate is **pure**: it performs no I/O, opens no sockets and reads no
//! clock. It only encodes and decodes the bytes that [`agent-client`] puts on
//! the wire.
//!
//! Only protocol **v1** is implemented: plain JSON, no envelope and no
//! compression. [`ReportV1`] is sent to `/api/clients/report` and
//! [`BasicInfoV1`] to `/api/clients/uploadBasicInfo`.
//!
//! Protocol v2 (JSON-RPC over `/api/clients/v2/rpc`) is intentionally not
//! implemented: the Go panel has no such route, so v2 is a dead path for this
//! project. See the crate README before adding it back.
//!
//! # Field compatibility
//!
//! Field names, types and optionality follow the Go implementation
//! (`komari-agent/monitoring`, `komari-agent/server`) and the panel's decoding
//! structs (`common.Report`, `common.ClientInfo`). Unknown fields are accepted
//! everywhere and preserved in the v1 models, because the Go agent builds those
//! payloads from `map[string]interface{}`.
//!
//! # Numbers
//!
//! Byte sizes, uptime and connection counters are unsigned 64-bit integers on
//! the agent side (`uint64`), while the panel stores the same values in
//! `int64`. Both are encoded as JSON integers. [`serde_json`] keeps integers
//! exact up to `u64::MAX`; no value is routed through `f64`.
//!
//! [`agent-client`]: https://docs.rs/agent-client

mod error;
pub mod v1;

pub use error::ProtocolError;
pub use v1::{
    BasicInfoV1, ConnectionsReportV1, CpuReportV1, DiskReportV1, GpuDeviceInfoV1, GpuReportV1,
    LoadReportV1, NetworkReportV1, RamReportV1, ReportV1,
};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Encode a protocol v1 model as compact JSON.
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

/// Decode a protocol v1 model from JSON.
///
/// Unknown fields are ignored unless the target model keeps them (the v1
/// models do, via a flattened `extra` map).
pub fn decode_v1<T: DeserializeOwned>(json: &str) -> Result<T, ProtocolError> {
    Ok(serde_json::from_str(json)?)
}
