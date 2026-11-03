//! Dashboard transport and reporting loop for the Komari agent.
//!
//! This crate implements the protocol **v1** runtime chain:
//!
//! * [`endpoint`] — WebSocket and basic-info URL construction (scheme rewrite,
//!   IDN, trailing slash, IPv6);
//! * [`ws`] — injectable WebSocket transport with custom DNS, TLS policy and
//!   Cloudflare Access headers;
//! * [`report`] — A03 metrics to `ReportV1` mapping;
//! * [`basic_info`] — `BasicInfoV1` payload and the Go compatibility retry;
//! * [`ip`] — the IP-only collector (A03 does not provide IP addresses);
//! * [`client`] — connect/send/heartbeat/drain/reconnect loop.
//!
//! v2, gzip and the POST fallback are intentionally absent (see
//! `docs/parity-deviations.md`, D-004).

pub mod basic_info;
pub mod client;
pub mod clock;
pub mod endpoint;
pub mod ip;
pub mod provider;
pub mod report;
pub mod ws;

pub use basic_info::{BasicInfoUploader, UploadError, UploadOutcome, build_basic_info};
pub use client::{AgentClient, ClientOptions, HEARTBEAT_INTERVAL, RunOutcome};
pub use clock::{Clock, TokioClock};
pub use endpoint::{
    BASIC_INFO_PATH, EndpointError, REPORT_PATH, build_basic_info_endpoint, build_ws_endpoint,
};
pub use ip::{IpOptions, addresses as ip_addresses};
pub use provider::{MonitorReportProvider, ReportProvider};
pub use report::{ReportSample, build_report};
pub use ws::{
    TungsteniteTransport, WsClientConfig, WsConnection, WsError, WsEvent, WsReceiver, WsSender,
    WsTransport,
};
