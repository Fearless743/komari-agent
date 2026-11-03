//! Error type shared by the monitor crate.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors returned by the low level `/proc` readers.
///
/// Collection entry points on [`crate::Collector`] deliberately fall back to
/// platform APIs instead of surfacing these errors, so the error type is mostly
/// useful for callers that read a specific file directly.
#[derive(Debug, Error)]
pub enum MonitorError {
    /// Reading a file below the configured proc root failed.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A file was readable but its contents did not match the expected format.
    #[error("failed to parse {path}: {reason}")]
    Parse { path: PathBuf, reason: String },
}

impl MonitorError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn parse(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Parse {
            path: path.into(),
            reason: reason.into(),
        }
    }
}
