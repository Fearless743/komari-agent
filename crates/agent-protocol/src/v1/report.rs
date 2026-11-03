//! Protocol v1 report model.
//!
//! Field names, types and `omitempty` behaviour mirror the Go agent's
//! dynamic `map[string]interface{}` payload (`monitoring.GenerateReport`) and
//! the server-side `common.Report` struct used to decode it.
//!
//! The Go agent builds the report as a map, so third-party or newer fields
//! survive a round trip. The same is achieved here with `#[serde(flatten)]`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A full protocol v1 metrics report (`/api/clients/report`).
///
/// Top-level fields follow the server's `common.Report` declaration order.
/// All non-optional fields have `#[serde(default)]` because Go's
/// `encoding/json` never rejects a report with missing keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportV1 {
    /// Client UUID. Optional on the wire: the Go agent never sends it and the
    /// server derives it from the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default)]
    pub cpu: CpuReportV1,
    #[serde(default)]
    pub ram: RamReportV1,
    #[serde(default)]
    pub swap: RamReportV1,
    #[serde(default)]
    pub load: LoadReportV1,
    #[serde(default)]
    pub disk: DiskReportV1,
    #[serde(default)]
    pub network: NetworkReportV1,
    #[serde(default)]
    pub connections: ConnectionsReportV1,
    /// Detailed GPU information. The Go agent only emits this section when
    /// `--enable-gpu` is set and detailed probing succeeded, hence `omitempty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuDetailReportV1>,
    #[serde(default)]
    pub uptime: u64,
    #[serde(default)]
    pub process: i64,
    /// Free-form diagnostics. Unlike most Go fields this one has **no**
    /// `omitempty`, so the agent always sends `"message": ""`.
    #[serde(default)]
    pub message: String,
    /// Server-assigned transport method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Server-assigned timestamp (RFC 3339). Kept as a string so the original
    /// Go timestamp formatting is preserved byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Unknown fields are preserved, matching the Go agent's map based payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `cpu` section.
///
/// On the server every field is `omitempty`; the Go agent currently only sends
/// `usage`, which is clamped to at least `0.001`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuReportV1 {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub cores: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arch: String,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub usage: f64,
}

/// `ram` / `swap` section. Go uses `uint64` in the agent and `int64` on the
/// server; both encode as unsigned decimal JSON integers for realistic sizes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RamReportV1 {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub used: u64,
}

/// `load` section (`load1`, `load5`, `load15`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadReportV1 {
    #[serde(default)]
    pub load1: f64,
    #[serde(default)]
    pub load5: f64,
    #[serde(default)]
    pub load15: f64,
}

/// `disk` section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskReportV1 {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub used: u64,
}

/// `network` section. Note the camelCase `totalUp` / `totalDown` keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkReportV1 {
    #[serde(default)]
    pub up: u64,
    #[serde(default)]
    pub down: u64,
    #[serde(default, rename = "totalUp")]
    pub total_up: u64,
    #[serde(default, rename = "totalDown")]
    pub total_down: u64,
}

/// `connections` section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionsReportV1 {
    #[serde(default)]
    pub tcp: i64,
    #[serde(default)]
    pub udp: i64,
}

/// `gpu` section in detailed mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuDetailReportV1 {
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub average_usage: f64,
    #[serde(default)]
    pub detailed_info: Vec<GpuDeviceInfoV1>,
}

/// One entry of `gpu.detailed_info`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuDeviceInfoV1 {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub memory_total: i64,
    #[serde(default)]
    pub memory_used: i64,
    #[serde(default)]
    pub utilization: f64,
    #[serde(default)]
    pub temperature: i64,
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

impl ReportV1 {
    /// Encode as compact JSON, equivalent to the Go agent's `json.Marshal`.
    pub fn to_json(&self) -> Result<String, crate::ProtocolError> {
        crate::encode_v1(self)
    }

    /// Decode from JSON, tolerating unknown fields.
    pub fn from_json(json: &str) -> Result<Self, crate::ProtocolError> {
        crate::decode_v1(json)
    }

    /// Encode as UTF-8 JSON bytes, ready for WebSocket text frames.
    pub fn to_json_vec(&self) -> Result<Vec<u8>, crate::ProtocolError> {
        Ok(self.to_json()?.into_bytes())
    }
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
}
