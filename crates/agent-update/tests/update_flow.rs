//! End-to-end update tests with an injected HTTP transport.
//!
//! Downloads are served from memory and replacements always target a
//! `tempfile::tempdir()`, never the test process executable.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_update::{
    HttpRequest, HttpResponse, Platform, UpdateError, UpdateHttp, UpdateOptions, UpdateOutcome,
    Updater,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug)]
enum Reply {
    Response(u16, Vec<u8>),
    Error(String),
}

struct FakeHttp {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl FakeHttp {
    fn new(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl UpdateHttp for FakeHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, UpdateError> {
        self.requests.lock().unwrap().push(request);

        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected HTTP request");

        match reply {
            Reply::Response(status, body) => Ok(HttpResponse { status, body }),
            Reply::Error(message) => Err(UpdateError::Http(message)),
        }
    }
}

fn release_value(
    tag: &str,
    assets: &[(&str, u64, Option<&str>)],
    draft: bool,
    prerelease: bool,
) -> Value {
    let assets: Vec<Value> = assets
        .iter()
        .map(|(name, size, digest)| {
            json!({
                "name": name,
                "browser_download_url": format!("https://downloads.example.com/{name}"),
                "size": size,
                "digest": digest,
            })
        })
        .collect();

    json!({
        "tag_name": tag,
        "name": tag,
        "draft": draft,
        "prerelease": prerelease,
        "assets": assets,
    })
}

fn releases_json(releases: &[Value]) -> Vec<u8> {
    serde_json::to_vec(releases).unwrap()
}

fn release_json(tag: &str, assets: &[(&str, u64, Option<&str>)]) -> Vec<u8> {
    releases_json(&[release_value(tag, assets, false, false)])
}

fn options(current: &str, target: Option<PathBuf>) -> UpdateOptions {
    UpdateOptions {
        repo: "owner/repo".to_owned(),
        current_version: current.to_owned(),
        disable_auto_update: false,
        platform: Platform::new("linux", "amd64"),
        target_path: target,
        ..UpdateOptions::default()
    }
}

fn write_target(directory: &tempfile::TempDir) -> PathBuf {
    let target = directory.path().join("komari-agent");
    fs::write(&target, b"old binary").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
    }

    target
}

fn assert_unchanged(target: &Path) {
    assert_eq!(fs::read(target).unwrap(), b"old binary");
}

#[tokio::test]
async fn disabled_short_circuits_before_any_request() {
    let http = FakeHttp::new(Vec::new());
    let updater = Updater::new(
        UpdateOptions {
            disable_auto_update: true,
            ..options("1.0.0", None)
        },
        http.clone(),
    )
    .unwrap();

    let outcome = updater.check_and_update().await.unwrap();

    assert_eq!(outcome, UpdateOutcome::Disabled);
    assert!(http.requests().is_empty());
}

#[tokio::test]
async fn check_latest_parses_release_and_selects_platform_asset() {
    let http = FakeHttp::new(vec![Reply::Response(
        200,
        release_json(
            "v1.5.0",
            &[
                ("komari-agent-darwin-arm64", 10, None),
                ("komari-agent-linux-amd64", 20, Some("sha256:abcd")),
                ("komari-agent-linux-arm64", 30, None),
            ],
        ),
    )]);
    let updater = Updater::new(options("1.0.0", None), http.clone()).unwrap();

    let latest = updater.check_latest().await.unwrap();

    assert_eq!(latest.version.to_string(), "1.5.0");
    assert_eq!(latest.asset.name, "komari-agent-linux-amd64");
    assert_eq!(latest.asset.size, 20);

    let requests = http.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url,
        "https://api.github.com/repos/owner/repo/releases?per_page=100"
    );
}

#[tokio::test]
async fn selects_highest_stable_release_skipping_draft_and_prerelease() {
    let asset = "komari-agent-linux-amd64";
    let http = FakeHttp::new(vec![Reply::Response(
        200,
        releases_json(&[
            release_value("v2.0.0", &[(asset, 10, None)], false, false),
            release_value("v9.0.0", &[(asset, 10, None)], true, false),
            release_value("v8.0.0", &[(asset, 10, None)], false, true),
            release_value("v3.0.0", &[(asset, 10, None)], false, false),
        ]),
    )]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let latest = updater.check_latest().await.unwrap();
    assert_eq!(latest.version.to_string(), "3.0.0");
}

#[tokio::test]
async fn skips_newer_releases_without_platform_asset() {
    let http = FakeHttp::new(vec![Reply::Response(
        200,
        releases_json(&[
            release_value(
                "v4.0.0",
                &[("komari-agent-darwin-arm64", 10, None)],
                false,
                false,
            ),
            release_value(
                "v2.0.0",
                &[("komari-agent-linux-amd64", 10, None)],
                false,
                false,
            ),
        ]),
    )]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let latest = updater.check_latest().await.unwrap();
    assert_eq!(latest.version.to_string(), "2.0.0");
}

#[tokio::test]
async fn empty_release_list_reports_no_release() {
    let http = FakeHttp::new(vec![Reply::Response(200, releases_json(&[]))]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let error = updater.check_latest().await.unwrap_err();
    assert!(matches!(error, UpdateError::NoRelease { .. }), "{error:?}");
}

#[tokio::test]
async fn disabled_updater_does_not_parse_invalid_current_version() {
    let http = FakeHttp::new(Vec::new());
    let updater = Updater::new(
        UpdateOptions {
            disable_auto_update: true,
            ..options("not-a-version", None)
        },
        http.clone(),
    )
    .unwrap();

    let outcome = updater.check_and_update().await.unwrap();
    assert_eq!(outcome, UpdateOutcome::Disabled);
    assert!(http.requests().is_empty());
}

#[tokio::test]
async fn up_to_date_release_does_not_download() {
    let http = FakeHttp::new(vec![Reply::Response(
        200,
        release_json("1.0.0", &[("komari-agent-linux-amd64", 10, None)]),
    )]);
    let updater = Updater::new(options("1.0.0", None), http.clone()).unwrap();

    let outcome = updater.check_and_update().await.unwrap();

    assert!(matches!(outcome, UpdateOutcome::UpToDate { .. }));
    assert_eq!(http.requests().len(), 1);
}

#[tokio::test]
async fn reports_non_2xx_release_response() {
    let http = FakeHttp::new(vec![Reply::Response(403, b"rate limited".to_vec())]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let error = updater.check_latest().await.unwrap_err();

    match error {
        UpdateError::HttpStatus { status, body, .. } => {
            assert_eq!(status, 403);
            assert!(body.contains("rate limited"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn rejects_invalid_release_json() {
    let http = FakeHttp::new(vec![Reply::Response(200, b"not json".to_vec())]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let error = updater.check_latest().await.unwrap_err();
    assert!(matches!(error, UpdateError::InvalidJson(_)), "{error:?}");
}

#[tokio::test]
async fn reports_missing_platform_asset() {
    let http = FakeHttp::new(vec![Reply::Response(
        200,
        release_json("2.0.0", &[("komari-agent-windows-amd64.exe", 10, None)]),
    )]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let error = updater.check_latest().await.unwrap_err();
    assert!(matches!(error, UpdateError::NoAsset { .. }), "{error:?}");
}

#[tokio::test]
async fn size_mismatch_keeps_existing_binary() {
    let directory = tempfile::tempdir().unwrap();
    let target = write_target(&directory);
    let http = FakeHttp::new(vec![
        Reply::Response(
            200,
            release_json("2.0.0", &[("komari-agent-linux-amd64", 999, None)]),
        ),
        Reply::Response(200, b"short".to_vec()),
    ]);
    let updater = Updater::new(options("1.0.0", Some(target.clone())), http).unwrap();

    let error = updater.check_and_update().await.unwrap_err();

    assert!(
        matches!(
            error,
            UpdateError::SizeMismatch {
                expected: 999,
                actual: 5
            }
        ),
        "{error:?}"
    );
    assert_unchanged(&target);
}

#[tokio::test]
async fn checksum_mismatch_keeps_existing_binary() {
    let directory = tempfile::tempdir().unwrap();
    let target = write_target(&directory);
    let body = b"\x7fELFnew-binary".to_vec();
    let http = FakeHttp::new(vec![
        Reply::Response(
            200,
            release_json(
                "2.0.0",
                &[(
                    "komari-agent-linux-amd64",
                    body.len() as u64,
                    Some("sha256:0000000000000000000000000000000000000000000000000000000000000000"),
                )],
            ),
        ),
        Reply::Response(200, body),
    ]);
    let updater = Updater::new(options("1.0.0", Some(target.clone())), http).unwrap();

    let error = updater.check_and_update().await.unwrap_err();

    assert!(
        matches!(error, UpdateError::ChecksumMismatch { .. }),
        "{error:?}"
    );
    assert_unchanged(&target);
}

#[tokio::test]
async fn download_transport_failure_keeps_existing_binary() {
    let directory = tempfile::tempdir().unwrap();
    let target = write_target(&directory);
    let http = FakeHttp::new(vec![
        Reply::Response(
            200,
            release_json("2.0.0", &[("komari-agent-linux-amd64", 4, None)]),
        ),
        Reply::Error("connection reset".to_owned()),
    ]);
    let updater = Updater::new(options("1.0.0", Some(target.clone())), http).unwrap();

    let error = updater.check_and_update().await.unwrap_err();

    assert!(matches!(error, UpdateError::Http(_)), "{error:?}");
    assert_unchanged(&target);
}

#[tokio::test]
async fn successful_update_replaces_binary_and_verifies_digest() {
    let directory = tempfile::tempdir().unwrap();
    let target = write_target(&directory);
    let body = b"\x7fELF\x02\x01\x01\x00new-binary".to_vec();
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&body)));

    let http = FakeHttp::new(vec![
        Reply::Response(
            200,
            release_json(
                "v2.0.0",
                &[(
                    "komari-agent-linux-amd64",
                    body.len() as u64,
                    Some(digest.as_str()),
                )],
            ),
        ),
        Reply::Response(200, body.clone()),
    ]);
    let updater = Updater::new(options("1.0.0", Some(target.clone())), http.clone()).unwrap();

    let outcome = updater.check_and_update().await.unwrap();

    match outcome {
        UpdateOutcome::Updated {
            previous,
            latest,
            target: replaced,
        } => {
            assert_eq!(previous.to_string(), "1.0.0");
            assert_eq!(latest.to_string(), "2.0.0");
            assert_eq!(replaced, target);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    assert_eq!(fs::read(&target).unwrap(), body);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o751);
    }

    let leftovers: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".new-"))
        .collect();
    assert!(leftovers.is_empty(), "temporary file left behind");

    let requests = http.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].url,
        "https://downloads.example.com/komari-agent-linux-amd64"
    );
}

#[tokio::test]
async fn rejects_html_download_and_too_large_payload() {
    let directory = tempfile::tempdir().unwrap();
    let target = write_target(&directory);

    let html = b"<!DOCTYPE html><html>blocked</html>".to_vec();
    let http = FakeHttp::new(vec![
        Reply::Response(
            200,
            release_json(
                "2.0.0",
                &[("komari-agent-linux-amd64", html.len() as u64, None)],
            ),
        ),
        Reply::Response(200, html),
    ]);
    let updater = Updater::new(options("1.0.0", Some(target.clone())), http).unwrap();

    let error = updater.check_and_update().await.unwrap_err();
    assert!(matches!(error, UpdateError::InvalidBinary), "{error:?}");
    assert_unchanged(&target);

    let large = vec![0x7f; 64];
    let http = FakeHttp::new(vec![
        Reply::Response(
            200,
            release_json(
                "2.0.0",
                &[("komari-agent-linux-amd64", large.len() as u64, None)],
            ),
        ),
        Reply::Response(200, large),
    ]);
    let mut settings = options("1.0.0", Some(target.clone()));
    settings.max_download_size = 16;
    let updater = Updater::new(settings, http).unwrap();

    let error = updater.check_and_update().await.unwrap_err();
    assert!(
        matches!(
            error,
            UpdateError::TooLarge {
                limit: 16,
                actual: 64
            }
        ),
        "{error:?}"
    );
    assert_unchanged(&target);
}

#[tokio::test]
async fn missing_target_is_rejected_after_version_check() {
    let http = FakeHttp::new(vec![Reply::Response(
        200,
        release_json("2.0.0", &[("komari-agent-linux-amd64", 4, None)]),
    )]);
    let updater = Updater::new(options("1.0.0", None), http).unwrap();

    let error = updater.check_and_update().await.unwrap_err();
    assert!(
        matches!(error, UpdateError::TargetNotConfigured),
        "{error:?}"
    );
}
