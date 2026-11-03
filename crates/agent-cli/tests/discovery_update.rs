//! Discovery, update and runtime wiring tests.
//!
//! HTTP is fully injected: no test talks to a real panel or GitHub.

use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};

use agent_cli::resolve_config;
use agent_config::Config;
use agent_discovery::{DiscoveryError, HttpClient, HttpRequest, HttpResponse};
use agent_update::{
    HttpRequest as UpdateRequest, HttpResponse as UpdateResponse, Platform, RESTART_EXIT_CODE,
    UpdateError, UpdateHttp, UpdateOptions, Updater,
};
use async_trait::async_trait;
use serde_json::json;

#[derive(Default)]
struct DiscoveryHttp {
    requests: Mutex<Vec<HttpRequest>>,
    replies: Mutex<VecDeque<HttpResponse>>,
}

#[async_trait]
impl HttpClient for DiscoveryHttp {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, DiscoveryError> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(HttpResponse {
                status: 200,
                body: Vec::new(),
            }))
    }
}

fn config(key: &str, token: &str, endpoint: &str) -> Config {
    Config {
        auto_discovery_key: key.to_owned(),
        token: token.to_owned(),
        endpoint: endpoint.to_owned(),
        ..Config::default()
    }
}

#[tokio::test]
async fn auto_discovery_registers_and_persists_credentials() {
    let http = Arc::new(DiscoveryHttp::default());
    http.replies.lock().unwrap().push_back(HttpResponse {
        status: 200,
        body: serde_json::to_vec(&json!({
            "status": "success",
            "message": "",
            "data": {"uuid": "uuid-1", "token": "registered-token"}
        }))
        .unwrap(),
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("auto-discovery.json");

    let resolved = resolve_config(
        config("discovery-key", "", "https://panel.example/"),
        Some(http.clone()),
        Some(path.clone()),
    )
    .await
    .unwrap();

    assert_eq!(resolved.token, "registered-token");
    assert_eq!(resolved.endpoint, "https://panel.example/");

    let requests = http.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert!(
        requests[0]
            .url
            .starts_with("https://panel.example/api/clients/register?name="),
        "{}",
        requests[0].url
    );
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "Bearer discovery-key")
    );

    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["uuid"], "uuid-1");
    assert_eq!(persisted["token"], "registered-token");
}

#[tokio::test]
async fn existing_auto_discovery_file_skips_registration() {
    let http = Arc::new(DiscoveryHttp::default());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("auto-discovery.json");
    fs::write(&path, br#"{"uuid":"old-uuid","token":"old-token"}"#).unwrap();

    let resolved = resolve_config(
        config("discovery-key", "", "https://panel.example"),
        Some(http.clone()),
        Some(path),
    )
    .await
    .unwrap();

    assert_eq!(resolved.token, "old-token");
    assert!(http.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configured_token_skips_auto_discovery() {
    let http = Arc::new(DiscoveryHttp::default());
    let resolved = resolve_config(
        config("", "configured-token", "https://panel.example"),
        Some(http.clone()),
        None,
    )
    .await
    .unwrap();

    assert_eq!(resolved.token, "configured-token");
    assert!(http.requests.lock().unwrap().is_empty());
}

struct UpdateHttpFake {
    replies: Mutex<VecDeque<UpdateResponse>>,
    requests: Mutex<Vec<UpdateRequest>>,
}

#[async_trait]
impl UpdateHttp for UpdateHttpFake {
    async fn send(&self, request: UpdateRequest) -> Result<UpdateResponse, UpdateError> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected update request"))
    }
}

fn make_updater(
    disabled: bool,
    replies: Vec<UpdateResponse>,
    target: std::path::PathBuf,
) -> Updater {
    Updater::new(
        UpdateOptions {
            repo: "owner/repo".to_owned(),
            current_version: "1.0.0".to_owned(),
            disable_auto_update: disabled,
            platform: Platform::new("linux", "amd64"),
            target_path: Some(target),
            ..UpdateOptions::default()
        },
        Arc::new(UpdateHttpFake {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        }),
    )
    .unwrap()
}

#[tokio::test]
async fn updated_outcome_maps_to_exit_code_42() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("komari-agent");
    fs::write(&target, b"old").unwrap();

    let binary = b"\x7fELFmock-binary".to_vec();
    let release = serde_json::to_vec(&json!([{
        "tag_name": "v1.1.0",
        "name": "v1.1.0",
        "draft": false,
        "prerelease": false,
        "assets": [{
            "name": "komari-agent-linux-amd64",
            "browser_download_url": "https://downloads.example.com/agent",
            "size": binary.len(),
            "digest": null
        }]
    }]))
    .unwrap();

    let updater = Arc::new(make_updater(
        false,
        vec![
            UpdateResponse {
                status: 200,
                body: release.clone(),
            },
            UpdateResponse {
                status: 200,
                body: binary.clone(),
            },
        ],
        target.clone(),
    ));

    let code = agent_cli::runtime::run_initial_update(&updater).await;
    assert_eq!(code, Some(RESTART_EXIT_CODE));
    assert_eq!(fs::read(&target).unwrap(), b"\x7fELFmock-binary");

    // The periodic path uses the same outcome mapping after its six-hour
    // delay; tests inject a zero delay to exercise run_periodic.
    fs::write(&target, b"old").unwrap();
    let updater = Arc::new(make_updater(
        false,
        vec![
            UpdateResponse {
                status: 200,
                body: release,
            },
            UpdateResponse {
                status: 200,
                body: binary,
            },
        ],
        target.clone(),
    ));
    let code = agent_cli::runtime::run_update_loop_after(updater, std::time::Duration::ZERO).await;
    assert_eq!(code, Some(RESTART_EXIT_CODE));
}

#[tokio::test]
async fn disabled_update_short_circuits_without_any_request() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("komari-agent");
    fs::write(&target, b"old").unwrap();

    let updater = Arc::new(make_updater(true, Vec::new(), target));
    let code = agent_cli::runtime::run_update_loop_after(updater, std::time::Duration::ZERO).await;
    assert_eq!(code, None);
}
