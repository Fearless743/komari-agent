//! Library support for the `komari-agent` binary.
//!
//! Holds the version constants, tracing initialisation, the runtime assembly
//! (`discovery`, WebSocket client, update loop) and the two diagnostic
//! subcommands.

pub mod commands;
pub mod runtime;

use std::io::IsTerminal;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing_subscriber::EnvFilter;

pub use runtime::{
    RunExit, RuntimeDeps, RuntimeError, client_options, monitor_options, resolve_config, run_with,
    supplement_options,
};

/// Agent version.
///
/// Defaults to the Cargo package version. Set `KOMARI_AGENT_VERSION` at build
/// time to override it, for example:
///
/// ```text
/// KOMARI_AGENT_VERSION=1.2.3 cargo build --release
/// ```
pub const VERSION: &str = match option_env!("KOMARI_AGENT_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Optional VCS revision embedded at build time via
/// `KOMARI_AGENT_VERSION_HASH`.
pub const VERSION_HASH: &str = match option_env!("KOMARI_AGENT_VERSION_HASH") {
    Some(hash) => hash,
    None => "",
};

static TRACING_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Full version string, including the VCS revision when one was provided.
pub fn full_version() -> &'static str {
    static FULL: OnceLock<String> = OnceLock::new();

    FULL.get_or_init(|| {
        if VERSION_HASH.is_empty() {
            VERSION.to_owned()
        } else {
            format!("{VERSION} ({VERSION_HASH})")
        }
    })
}

/// Initialise logging once. Later calls are no-ops.
///
/// Honours `RUST_LOG` (defaulting to `info`) and writes to stderr, like the Go
/// agent's standard logger.
pub fn init_tracing() {
    if TRACING_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_defaults_to_cargo_package_version() {
        assert!(!VERSION.is_empty());
        assert_eq!(
            VERSION,
            option_env!("KOMARI_AGENT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn version_hash_matches_compile_time_override() {
        assert_eq!(
            VERSION_HASH,
            option_env!("KOMARI_AGENT_VERSION_HASH").unwrap_or("")
        );
    }

    #[test]
    fn full_version_has_the_expected_shape() {
        let expected = if VERSION_HASH.is_empty() {
            VERSION.to_owned()
        } else {
            format!("{VERSION} ({VERSION_HASH})")
        };

        assert_eq!(full_version(), expected);
    }
}
