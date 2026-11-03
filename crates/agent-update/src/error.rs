//! Update errors.

use std::path::PathBuf;

use thiserror::Error;

use crate::version::VersionError;

/// Errors returned by release checks, downloads and binary replacement.
#[derive(Debug, Error)]
pub enum UpdateError {
    #[error(transparent)]
    Version(#[from] VersionError),

    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("HTTP request to {url} failed with status {status}: {body}")]
    HttpStatus {
        url: String,
        status: u16,
        body: String,
    },

    #[error("failed to parse GitHub release: {0}")]
    InvalidJson(#[source] serde_json::Error),

    #[error("latest release has no tag_name")]
    MissingTag,

    #[error("no release asset found for {os}-{arch}")]
    NoAsset { os: String, arch: String },

    #[error("download is empty")]
    EmptyDownload,

    #[error("download size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("download exceeds the configured limit of {limit} bytes (got {actual})")]
    TooLarge { limit: u64, actual: u64 },

    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("download does not look like a binary")]
    InvalidBinary,

    #[error("no target path configured for the update")]
    TargetNotConfigured,

    #[error("failed to replace {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
