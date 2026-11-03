//! Protocol v1 report model.
//!
//! Field names, types and `omitempty` behaviour mirror the Go agent's
//! dynamic `map[string]interface{}` payload (`monitoring.GenerateReport`) and
//! the server-side `common.Report` struct used to decode it.
//!
//! The Go agent builds the report as a map, so third-party or newer fields
//! survive a round trip. The same is achieved for the top level and for every
//! section with `#[serde(flatten)]`. Go's `encoding/json` also accepts JSON
//! `null` for any field and leaves the zero value in place; the
//! [`null_default`](crate::v1::null_default) helper reproduces that.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A full protocol v1 metrics report (`/api/clients/report`).
///
/// Top-level fields follow the server's `common.Report` declaration order.
/// All non-optional fields accept a missing key or `null` and fall back to the
/// zero value, because Go's `encoding/json` never rejects such payloads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportV1 {
    /// Client UUID. Optional on the wire: the Go agent never sends it and the
    /// server derives it from the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub cpu: CpuReportV1,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub ram: RamReportV1,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub swap: RamReportV1,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub load: LoadReportV1,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub disk: DiskReportV1,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub network: NetworkReportV1,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub connections: ConnectionsReportV1,
    /// GPU information. The Go agent emits it only with `--enable-gpu`; it has
    /// two different shapes, see [`GpuReportV1`]. `null` means absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuReportV1>,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub uptime: u64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub process: i64,
    /// Free-form diagnostics. Unlike most Go fields this one has **no**
    /// `omitempty`, so the agent always sends `"message": ""`.
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub message: String,
    /// Server-assigned transport method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Server-assigned timestamp (RFC 3339). Kept as a string so the original
    /// Go timestamp formatting is preserved byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Unknown top-level fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ReportV1 {
    /// Encode as compact JSON, equivalent to the Go agent's `json.Marshal`.
    pub fn to_json(&self) -> Result<String, crate::ProtocolError> {
        crate::encode_v1(self)
    }

    /// Decode from JSON, tolerating unknown fields and `null` values.
    pub fn from_json(json: &str) -> Result<Self, crate::ProtocolError> {
        crate::decode_v1(json)
    }

    /// Encode as UTF-8 JSON bytes, ready for WebSocket text frames.
    pub fn to_json_vec(&self) -> Result<Vec<u8>, crate::ProtocolError> {
        Ok(self.to_json()?.into_bytes())
    }
}

/// `cpu` section.
///
/// On the server every field is `omitempty`; the Go agent currently only sends
/// `usage`, which is clamped to at least `0.001`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuReportV1 {
    #[serde(
        default,
        deserialize_with = "crate::v1::null_default::deserialize",
        skip_serializing_if = "String::is_empty"
    )]
    pub name: String,
    #[serde(
        default,
        deserialize_with = "crate::v1::null_default::deserialize",
        skip_serializing_if = "is_zero_i64"
    )]
    pub cores: i64,
    #[serde(
        default,
        deserialize_with = "crate::v1::null_default::deserialize",
        skip_serializing_if = "String::is_empty"
    )]
    pub arch: String,
    #[serde(
        default,
        deserialize_with = "crate::v1::null_default::deserialize",
        skip_serializing_if = "is_zero_f64"
    )]
    pub usage: f64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `ram` / `swap` section. Go uses `uint64` in the agent and `int64` on the
/// server; both encode as unsigned decimal JSON integers for realistic sizes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RamReportV1 {
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub total: u64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub used: u64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `load` section (`load1`, `load5`, `load15`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadReportV1 {
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub load1: f64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub load5: f64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub load15: f64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `disk` section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskReportV1 {
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub total: u64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub used: u64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `network` section. Note the camelCase `totalUp` / `totalDown` keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkReportV1 {
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub up: u64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub down: u64,
    #[serde(
        default,
        rename = "totalUp",
        deserialize_with = "crate::v1::null_default::deserialize"
    )]
    pub total_up: u64,
    #[serde(
        default,
        rename = "totalDown",
        deserialize_with = "crate::v1::null_default::deserialize"
    )]
    pub total_down: u64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `connections` section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectionsReportV1 {
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub tcp: i64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub udp: i64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `gpu` section.
///
/// Go's `monitoring.GenerateReport` emits **two different shapes**:
///
/// * detailed probing succeeded:
///   `{"count":2,"average_usage":37.5,"detailed_info":[...]}`
/// * detailed probing failed but GPU model detection succeeded:
///   `{"models":["NVIDIA GeForce RTX 4090", ...]}`
///
/// All branches are `Option` so the exact shape (including whether a zero
/// value such as `average_usage: 0` was present) survives a round trip.
/// Unknown fields are preserved in [`GpuReportV1::extra`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuReportV1 {
    /// Only in the detailed shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    /// Only in the detailed shape. `Some(0.0)` is kept; `None` means absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_usage: Option<f64>,
    /// Only in the detailed shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detailed_info: Option<Vec<GpuDeviceInfoV1>>,
    /// Only in the model-only fallback shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One entry of `gpu.detailed_info`.
///
/// Go's `monitoring/unit.GPUInfo` uses `uint64` for memory and temperature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuDeviceInfoV1 {
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub name: String,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub memory_total: u64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub memory_used: u64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub utilization: f64,
    #[serde(default, deserialize_with = "crate::v1::null_default::deserialize")]
    pub temperature: u64,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_omitempty_matches_go() {
        let cpu = CpuReportV1 {
            usage: 0.001,
            ..CpuReportV1::default()
        };
        assert_eq!(serde_json::to_string(&cpu).unwrap(), r#"{"usage":0.001}"#);
    }

    #[test]
    fn gpu_models_fallback_keeps_only_models() {
        let gpu = GpuReportV1 {
            models: Some(vec!["NVIDIA GeForce RTX 4090".to_string()]),
            ..GpuReportV1::default()
        };
        assert_eq!(
            serde_json::to_string(&gpu).unwrap(),
            r#"{"models":["NVIDIA GeForce RTX 4090"]}"#
        );
    }

    #[test]
    fn null_scalar_becomes_zero() {
        let cpu: CpuReportV1 = serde_json::from_str(r#"{"usage":null}"#).unwrap();
        assert_eq!(cpu, CpuReportV1::default());
    }
}
