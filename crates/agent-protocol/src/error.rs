//! Error type shared by the protocol v1 encoders and decoders.

use thiserror::Error;

/// Errors returned by the pure protocol helpers in this crate.
///
/// The crate performs no I/O; the only failure mode is invalid JSON.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// JSON encoding or decoding failed.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}
