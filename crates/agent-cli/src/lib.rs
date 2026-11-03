//! Library support for the `komari-agent` binary.

/// Placeholder version string, replaced by the real version module.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
