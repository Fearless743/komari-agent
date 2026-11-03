//! Error types shared by the protocol encoders and the gzip transport helpers.

use std::io;

use thiserror::Error;

/// Errors returned by the pure protocol helpers in this crate.
///
/// The crate performs no I/O; all variants describe in-memory data problems.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// JSON encoding or decoding failed.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// gzip compression failed (only possible for an unusable writer).
    #[error("gzip compression failed: {0}")]
    Compress(io::Error),

    /// gzip decompression failed (corrupt input, bad CRC or wrong size).
    #[error("gzip decompression failed: {0}")]
    Decompress(io::Error),

    /// A gzip compression level outside `0..=9` was requested.
    #[error("invalid gzip compression level {0}, expected 0..=9")]
    InvalidLevel(u32),

    /// The input passed to [`crate::decompress`] was empty.
    ///
    /// Go's `gzip.NewReader` reports `io.EOF` for an empty buffer; we surface a
    /// dedicated variant so callers can distinguish it from corrupt data.
    #[error("gzip input is empty")]
    EmptyInput,
}
