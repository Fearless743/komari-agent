//! Self update flow.
//!
//! Mirrors `komari-agent/update/update.go`:
//!
//! * query the latest GitHub release;
//! * parse the tag as SemVer and compare it with the current version;
//! * select the asset for the current platform;
//! * download, validate size/optional SHA-256 and atomically replace the binary.
//!
//! The Go agent terminates with exit code 42 after installing an update so the
//! service manager can restart it. This crate only returns
//! [`UpdateOutcome::Updated`]; the CLI layer is responsible for exiting with
//! [`RESTART_EXIT_CODE`] when appropriate.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::UpdateError;
use crate::http::{HttpRequest, UpdateHttp};
use crate::release::{Asset, GithubRelease, Platform, select_asset};
use crate::replace::replace_file_atomically;
use semver::Version;

use crate::version::{need_update, parse_version};

/// Repository queried by default, matching the Go `update.Repo`.
pub const DEFAULT_REPO: &str = "fearless743/komari-agent";

/// GitHub API base URL.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// Go `DoUpdateWorks` checks every 6 hours.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Go exits with 42 after a successful self-update.
pub const RESTART_EXIT_CODE: i32 = 42;

/// Default maximum download size (256 MiB).
pub const DEFAULT_MAX_DOWNLOAD_SIZE: u64 = 256 * 1024 * 1024;

/// Version compiled into the agent.
///
/// `KOMARI_AGENT_VERSION` overrides the Cargo package version at build time,
/// matching the CLI's version constant and the Go linker flag.
pub const DEFAULT_CURRENT_VERSION: &str = match option_env!("KOMARI_AGENT_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Update configuration.
#[derive(Clone, Debug)]
pub struct UpdateOptions {
    pub repo: String,
    /// Current version string, with or without a `v` prefix.
    pub current_version: String,
    /// When true, [`Updater::check_and_update`] returns
    /// [`UpdateOutcome::Disabled`] without any network access.
    pub disable_auto_update: bool,
    pub platform: Platform,
    /// Binary to replace. `None` makes `check_and_update` fail with
    /// [`UpdateError::TargetNotConfigured`] (use `check_latest` for check-only).
    pub target_path: Option<PathBuf>,
    /// Maximum accepted download size; `0` disables the limit.
    pub max_download_size: u64,
    /// GitHub API base, overridable for GitHub Enterprise or tests.
    pub api_base: String,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            repo: DEFAULT_REPO.to_owned(),
            current_version: DEFAULT_CURRENT_VERSION.to_owned(),
            disable_auto_update: false,
            platform: Platform::current(),
            target_path: None,
            max_download_size: DEFAULT_MAX_DOWNLOAD_SIZE,
            api_base: DEFAULT_API_BASE.to_owned(),
        }
    }
}

/// Latest matching release and asset.
#[derive(Clone, Debug)]
pub struct LatestRelease {
    pub version: Version,
    pub tag: String,
    pub asset: Asset,
}

/// Result of a check/update run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// `disable_auto_update` was set; nothing was queried.
    Disabled,
    /// The current version is already the latest.
    UpToDate { current: Version, latest: Version },
    /// A new binary was installed.
    Updated {
        previous: Version,
        latest: Version,
        target: PathBuf,
    },
}

/// Release checker and updater.
pub struct Updater {
    http: Arc<dyn UpdateHttp>,
    options: UpdateOptions,
    current: Version,
}

impl Updater {
    /// Build an updater with an injected HTTP transport.
    pub fn new(options: UpdateOptions, http: Arc<dyn UpdateHttp>) -> Result<Self, UpdateError> {
        let current = parse_version(&options.current_version)?;
        Ok(Self {
            http,
            options,
            current,
        })
    }

    /// Build an updater with the production `reqwest` transport.
    pub fn with_default_http(
        options: UpdateOptions,
        timeout: Duration,
        ignore_unsafe_cert: bool,
    ) -> Result<Self, UpdateError> {
        let http = Arc::new(crate::http::ReqwestUpdateHttp::new(
            timeout,
            ignore_unsafe_cert,
        ));
        Self::new(options, http)
    }

    /// Parsed current version.
    pub fn current_version(&self) -> &Version {
        &self.current
    }

    /// Effective options.
    pub fn options(&self) -> &UpdateOptions {
        &self.options
    }

    /// Query the latest release and select the platform asset without
    /// downloading anything.
    pub async fn check_latest(&self) -> Result<LatestRelease, UpdateError> {
        let url = format!(
            "{}/repos/{}/releases/latest",
            self.options.api_base.trim_end_matches('/'),
            self.options.repo.trim_matches('/')
        );

        let response = self
            .http
            .send(HttpRequest {
                url: url.clone(),
                headers: vec![
                    (
                        "Accept".to_owned(),
                        "application/vnd.github+json".to_owned(),
                    ),
                    ("X-GitHub-Api-Version".to_owned(), "2022-11-28".to_owned()),
                ],
            })
            .await?;

        if response.status != 200 {
            return Err(UpdateError::HttpStatus {
                url,
                status: response.status,
                body: truncate_body(&response.body),
            });
        }

        let release: GithubRelease =
            serde_json::from_slice(&response.body).map_err(UpdateError::InvalidJson)?;

        if release.tag_name.trim().is_empty() {
            return Err(UpdateError::MissingTag);
        }

        let version = parse_version(&release.tag_name)?;
        let platform = &self.options.platform;
        let asset = select_asset(&release.assets, platform)
            .cloned()
            .ok_or_else(|| UpdateError::NoAsset {
                os: platform.os.clone(),
                arch: platform.arch.clone(),
            })?;

        Ok(LatestRelease {
            version,
            tag: release.tag_name,
            asset,
        })
    }

    /// Check for a newer release and install it when one exists.
    pub async fn check_and_update(&self) -> Result<UpdateOutcome, UpdateError> {
        if self.options.disable_auto_update {
            return Ok(UpdateOutcome::Disabled);
        }

        let latest = self.check_latest().await?;

        if !need_update(&self.current, &latest.version) {
            return Ok(UpdateOutcome::UpToDate {
                current: self.current.clone(),
                latest: latest.version,
            });
        }

        let target = self
            .options
            .target_path
            .clone()
            .ok_or(UpdateError::TargetNotConfigured)?;

        let response = self
            .http
            .send(HttpRequest {
                url: latest.asset.browser_download_url.clone(),
                headers: vec![("Accept".to_owned(), "application/octet-stream".to_owned())],
            })
            .await?;

        if response.status != 200 {
            return Err(UpdateError::HttpStatus {
                url: latest.asset.browser_download_url.clone(),
                status: response.status,
                body: truncate_body(&response.body),
            });
        }

        let bytes = response.body;

        if bytes.is_empty() {
            return Err(UpdateError::EmptyDownload);
        }

        let actual = bytes.len() as u64;
        if self.options.max_download_size > 0 && actual > self.options.max_download_size {
            return Err(UpdateError::TooLarge {
                limit: self.options.max_download_size,
                actual,
            });
        }

        if latest.asset.size > 0 && actual != latest.asset.size {
            return Err(UpdateError::SizeMismatch {
                expected: latest.asset.size,
                actual,
            });
        }

        if let Some(digest) = &latest.asset.digest {
            verify_digest(digest, &bytes)?;
        }

        if looks_like_text(&bytes) {
            return Err(UpdateError::InvalidBinary);
        }

        replace_file_atomically(&target, &bytes)?;

        Ok(UpdateOutcome::Updated {
            previous: self.current.clone(),
            latest: latest.version,
            target,
        })
    }
}

fn verify_digest(digest: &str, bytes: &[u8]) -> Result<(), UpdateError> {
    let digest = digest.trim();
    let Some((algorithm, expected)) = digest.split_once(':') else {
        tracing::warn!(
            digest,
            "ignoring release digest without an algorithm prefix"
        );
        return Ok(());
    };

    if !algorithm.eq_ignore_ascii_case("sha256") {
        tracing::warn!(algorithm, "ignoring unsupported release digest algorithm");
        return Ok(());
    }

    let actual = hex::encode(Sha256::digest(bytes));
    if expected.trim().eq_ignore_ascii_case(&actual) {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch {
            expected: expected.trim().to_owned(),
            actual,
        })
    }
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let rest = &bytes[start..];

    rest.starts_with(b"<!DOCTYPE")
        || rest.starts_with(b"<html")
        || rest.starts_with(b"<?xml")
        || rest.starts_with(b"{")
}

fn truncate_body(body: &[u8]) -> String {
    const LIMIT: usize = 512;
    let text = String::from_utf8_lossy(body);
    if text.chars().count() > LIMIT {
        let truncated: String = text.chars().take(LIMIT).collect();
        format!("{truncated}…")
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_helpers_reject_error_pages() {
        assert!(looks_like_text(b"<!DOCTYPE html><html></html>"));
        assert!(looks_like_text(b"  <html>"));
        assert!(looks_like_text(b"{\"message\":\"rate limited\"}"));
        assert!(!looks_like_text(b"\x7fELF\x02\x01\x01"));
        assert!(!looks_like_text(b"MZ\x90\x00"));
    }

    #[test]
    fn sha256_digest_is_verified() {
        // sha256("hello") = 2cf24dba...
        let bytes = b"hello";
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        verify_digest(&digest, bytes).unwrap();

        let error = verify_digest(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            bytes,
        )
        .unwrap_err();
        assert!(matches!(error, UpdateError::ChecksumMismatch { .. }));

        // Unsupported algorithms and unprefixed digests are ignored.
        verify_digest("sha512:abcd", bytes).unwrap();
        verify_digest("abcd", bytes).unwrap();
    }
}
