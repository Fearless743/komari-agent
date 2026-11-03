//! Protocol v1 basic information model.
//!
//! The Go agent builds this payload as a `map[string]interface{}` in
//! `server.uploadBasicInfo` and posts it to `/api/clients/uploadBasicInfo`.
//! Two keys are dropped when the panel rejects the full payload:
//!
//! * `kernel_version` (panels <= 1.0.2)
//! * `cpu_physical_cores` (panels <= 1.2.0)
//!
//! Both are modelled as `Option` so "missing" can be expressed exactly.
//! Everything else mirrors the Go map, including empty strings such as
//! `"ipv6": ""`, which the agent does send.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Static host information as reported by the agent.
///
/// Unknown/third-party keys are preserved via [`BasicInfoV1::extra`], matching
/// the Go map payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasicInfoV1 {
    #[serde(default)]
    pub cpu_name: String,
    #[serde(default)]
    pub cpu_cores: i64,
    /// Dropped for panels <= 1.2.0; `None` serialises as absent, like
    /// `delete(data, "cpu_physical_cores")` in Go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_physical_cores: Option<i64>,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub os: String,
    /// Dropped for panels <= 1.0.2; `None` serialises as absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_version: Option<String>,
    #[serde(default)]
    pub ipv4: String,
    #[serde(default)]
    pub ipv6: String,
    #[serde(default)]
    pub mem_total: u64,
    #[serde(default)]
    pub swap_total: u64,
    #[serde(default)]
    pub disk_total: u64,
    #[serde(default)]
    pub gpu_name: String,
    #[serde(default)]
    pub virtualization: String,
    #[serde(default)]
    pub version: String,
    /// Injected by the panel from the request token, never sent by the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// Injected by the panel when GeoIP is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Unknown fields are preserved, matching the Go map payload.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for BasicInfoV1 {
    /// A "full" payload equivalent to the Go agent's first upload attempt.
    ///
    /// Go always includes `cpu_physical_cores` (possibly `0`) and
    /// `kernel_version` (possibly `""`); those keys are only deleted when the
    /// panel rejects the full payload. Use `None` explicitly to emulate that
    /// fallback.
    fn default() -> Self {
        Self {
            cpu_name: String::new(),
            cpu_cores: 0,
            cpu_physical_cores: Some(0),
            arch: String::new(),
            os: String::new(),
            kernel_version: Some(String::new()),
            ipv4: String::new(),
            ipv6: String::new(),
            mem_total: 0,
            swap_total: 0,
            disk_total: 0,
            gpu_name: String::new(),
            virtualization: String::new(),
            version: String::new(),
            uuid: None,
            region: None,
            extra: BTreeMap::new(),
        }
    }
}

impl BasicInfoV1 {
    /// Encode as compact JSON, equivalent to the Go agent's `json.Marshal`.
    pub fn to_json(&self) -> Result<String, crate::ProtocolError> {
        crate::encode_v1(self)
    }

    /// Decode from JSON, tolerating unknown fields.
    pub fn from_json(json: &str) -> Result<Self, crate::ProtocolError> {
        crate::decode_v1(json)
    }
}
