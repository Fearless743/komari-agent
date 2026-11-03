//! Protocol v1 wire models.
//!
//! v1 is plain JSON: the agent POSTs the report object to
//! `/api/clients/report` (or sends it as a WebSocket text frame) and the basic
//! information object to `/api/clients/uploadBasicInfo`. There is no envelope,
//! no JSON-RPC and no compression for v1 reports.

mod basic_info;
mod report;

pub use basic_info::BasicInfoV1;
pub use report::{
    ConnectionsReportV1, CpuReportV1, DiskReportV1, GpuDetailReportV1, GpuDeviceInfoV1,
    LoadReportV1, NetworkReportV1, RamReportV1, ReportV1,
};
