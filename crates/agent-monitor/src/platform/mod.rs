//! Platform fallbacks.
//!
//! Linux is the full implementation. Other platforms, and Linux runs where the
//! proc files are unavailable, use `sysinfo` for the same values. Keeping the
//! fallback compiled on Linux means its API usage is checked by the normal test
//! suite.

pub(crate) mod fallback;
