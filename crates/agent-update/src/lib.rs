//! Self update support for the Komari agent.
//!
//! The crate is independent of WebSocket and metric collection code. It
//! exposes an injectable HTTP transport so tests can exercise release checks,
//! download validation and atomic replacement without touching the network or
//! the running test binary.
//!
//! Typical wiring:
//!
//! ```no_run
//! # async fn run() -> Result<(), agent_update::UpdateError> {
//! use agent_update::{UpdateOptions, Updater};
//!
//! let options = UpdateOptions {
//!     target_path: Some(std::env::current_exe().unwrap()),
//!     ..UpdateOptions::default()
//! };
//! let updater = Updater::with_default_http(options, std::time::Duration::from_secs(60), false)?;
//! let outcome = updater.check_and_update().await?;
//! # let _ = outcome;
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod http;
pub mod release;
pub mod replace;
pub mod update;
pub mod version;

pub use error::UpdateError;
pub use http::{HttpRequest, HttpResponse, ReqwestUpdateHttp, UpdateHttp};
pub use release::{Asset, GithubRelease, Platform, select_asset};
pub use replace::replace_file_atomically;
pub use update::{
    CHECK_INTERVAL, DEFAULT_API_BASE, DEFAULT_MAX_DOWNLOAD_SIZE, DEFAULT_REPO, LatestRelease,
    RESTART_EXIT_CODE, UpdateOptions, UpdateOutcome, Updater,
};
pub use version::{VersionError, need_update, parse_version};

/// Re-export of the SemVer type used across the crate.
pub use semver::Version;
