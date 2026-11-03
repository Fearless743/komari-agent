//! Protocol v2 JSON-RPC 2.0 models.
//!
//! v2 wraps the same v1 payloads in JSON-RPC envelopes:
//!
//! ```json
//! {"jsonrpc":"2.0","method":"agent.report","params":{"report":{...}}}
//! {"jsonrpc":"2.0","method":"agent.basicInfo","params":{"info":{...}}}
//! ```
//!
//! The shapes mirror the Go agent's `protocol/v2` package and the panel's
//! `utils/rpc` types (`JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`).
//! Requests without an `id` are notifications; the panel must not answer them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ProtocolError;
use crate::v1::{BasicInfoV1, ReportV1};

/// JSON-RPC version string used by both the agent and the panel.
pub const JSONRPC_VERSION: &str = "2.0";

/// Alias matching the Go constant name for convenience.
pub const RPC_VERSION: &str = JSONRPC_VERSION;

/// Method name for periodic metric reports.
pub const METHOD_AGENT_REPORT: &str = "agent.report";

/// Method name for static host information uploads.
pub const METHOD_AGENT_BASIC_INFO: &str = "agent.basicInfo";

// JSON-RPC 2.0 standard error codes (Go `utils/rpc/errors.go`).
/// Invalid JSON was received.
pub const PARSE_ERROR: i64 = -32700;
/// The JSON was valid but not a valid Request object.
pub const INVALID_REQUEST: i64 = -32600;
/// The method does not exist or is unavailable.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid method parameters.
pub const INVALID_PARAMS: i64 = -32602;
/// Internal server error.
pub const INTERNAL_ERROR: i64 = -32603;

// Komari-specific error codes (Go `utils/rpc/errors.go`).
/// Requested resource not found.
pub const NOT_FOUND: i64 = -32044;
/// Resource already exists.
pub const ALREADY_EXISTS: i64 = -32045;
/// Caller lacks permission.
pub const PERMISSION_DENIED: i64 = -32041;
/// Caller is not authenticated.
pub const UNAUTHENTICATED: i64 = -32040;
/// The request was cancelled.
pub const CANCELLED: i64 = -32010;
/// The request deadline was exceeded.
pub const DEADLINE_EXCEEDED: i64 = -32011;
/// Concurrent conflict or aborted transaction.
pub const ABORTED: i64 = -32021;
/// Numeric/index out of range.
pub const OUT_OF_RANGE: i64 = -32022;
/// Feature not implemented.
pub const UNIMPLEMENTED: i64 = -32050;
/// Dependent service is unavailable.
pub const UNAVAILABLE: i64 = -32051;
/// Unrecoverable data loss.
pub const DATA_LOSS: i64 = -32052;

/// A JSON-RPC 2.0 request or notification.
///
/// Go uses `interface{}` for `params` and `id`; here they are
/// [`serde_json::Value`] so arbitrary object/array/scalar params and
/// string/number/null ids are all preserved. Unknown members are ignored on
/// decode, exactly like Go's `encoding/json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Must be `"2.0"`; kept as a field so malformed requests can be decoded
    /// and rejected by [`RpcRequest::validate`] instead of failing to parse.
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Absent (or `null`) for notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
}

impl RpcRequest {
    /// Build a notification (no `id`).
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
            id: None,
        }
    }

    /// Build a request that expects a response.
    pub fn request(id: impl Into<Value>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
            id: Some(id.into()),
        }
    }

    /// `true` when no `id` is present, i.e. the panel must not reply.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// Validate `jsonrpc` and `method`, mirroring Go's `JsonRpcRequest.Validate`.
    ///
    /// Returns a ready-to-send [`RpcError`] with [`INVALID_REQUEST`] on failure.
    pub fn validate(&self) -> Result<(), RpcError> {
        if self.jsonrpc != JSONRPC_VERSION {
            return Err(RpcError::new(INVALID_REQUEST, "invalid jsonrpc version"));
        }
        if self.method.trim().is_empty() {
            return Err(RpcError::new(INVALID_REQUEST, "method required"));
        }
        Ok(())
    }

    /// Encode as compact JSON.
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        crate::encode_v1(self)
    }

    /// Decode from JSON, ignoring unknown members.
    pub fn from_json(json: &str) -> Result<Self, ProtocolError> {
        crate::decode_v1(json)
    }
}

/// A JSON-RPC 2.0 response.
///
/// Success responses carry `result`; failures carry `error`. Both are
/// `omitempty` in Go and therefore optional here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// Build a success response (`jsonrpc` set to `"2.0"`).
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response (`jsonrpc` set to `"2.0"`).
    pub fn failure(id: Option<Value>, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Encode as compact JSON.
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        crate::encode_v1(self)
    }

    /// Decode the body returned by the panel.
    pub fn from_json(json: &str) -> Result<Self, ProtocolError> {
        crate::decode_v1(json)
    }
}

/// JSON-RPC error object (`utils/rpc.JsonRpcError`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Build an error object without `data`.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach extension data.
    pub fn with_data(mut self, data: impl Into<Value>) -> Self {
        self.data = Some(data.into());
        self
    }
}

/// `params` object of an `agent.report` call: `{"report": <v1 report>}`.
///
/// The Go builder unmarshals the v1 bytes into `interface{}` before
/// re-marshalling, which rounds integers above 2^53 through `float64`. This
/// crate keeps the exact integer value instead; see the crate README.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentReportParams {
    pub report: Value,
}

/// `params` object of an `agent.basicInfo` call: `{"info": <basic info>}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentBasicInfoParams {
    pub info: Value,
}

/// Build the v2 `params` for a typed report.
pub fn report_params(report: &ReportV1) -> Result<AgentReportParams, ProtocolError> {
    Ok(AgentReportParams {
        report: serde_json::to_value(report)?,
    })
}

/// Build the v2 `params` for an already decoded report value.
pub fn report_params_from_value(report: Value) -> AgentReportParams {
    AgentReportParams { report }
}

/// Build the v2 `params` for typed basic information.
pub fn basic_info_params(info: &BasicInfoV1) -> Result<AgentBasicInfoParams, ProtocolError> {
    Ok(AgentBasicInfoParams {
        info: serde_json::to_value(info)?,
    })
}

/// Build an `agent.report` **notification** payload (WebSocket v2 path).
pub fn report_notification(report: &ReportV1) -> Result<String, ProtocolError> {
    let params = serde_json::to_value(report_params(report)?)?;
    RpcRequest::notification(METHOD_AGENT_REPORT, Some(params)).to_json()
}

/// Build an `agent.report` **request** payload (HTTP POST fallback path).
pub fn report_request(id: impl Into<Value>, report: &ReportV1) -> Result<String, ProtocolError> {
    let params = serde_json::to_value(report_params(report)?)?;
    RpcRequest::request(id, METHOD_AGENT_REPORT, Some(params)).to_json()
}

/// Build an `agent.basicInfo` **notification** payload.
pub fn basic_info_notification(info: &BasicInfoV1) -> Result<String, ProtocolError> {
    let params = serde_json::to_value(basic_info_params(info)?)?;
    RpcRequest::notification(METHOD_AGENT_BASIC_INFO, Some(params)).to_json()
}

/// Build an `agent.basicInfo` **request** payload.
pub fn basic_info_request(
    id: impl Into<Value>,
    info: &BasicInfoV1,
) -> Result<String, ProtocolError> {
    let params = serde_json::to_value(basic_info_params(info)?)?;
    RpcRequest::request(id, METHOD_AGENT_BASIC_INFO, Some(params)).to_json()
}

/// Convenience re-export of the raw JSON object map type used for params.
pub type ParamsMap = BTreeMap<String, Value>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_has_no_id() {
        let req = RpcRequest::notification(METHOD_AGENT_REPORT, None);
        assert!(req.is_notification());
        assert_eq!(
            req.to_json().unwrap(),
            r#"{"jsonrpc":"2.0","method":"agent.report"}"#
        );
    }
}
