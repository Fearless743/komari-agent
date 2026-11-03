//! `run_with` wiring tests with injected transports.

use agent_cli::runtime::{RunExit, RuntimeDeps, run_with};
use agent_client::{
    Clock, ReportProvider, ReportSample, TokioClock, WsConnection, WsError, WsTransport,
};
use agent_config::Config;
use agent_discovery::{DiscoveryError, HttpClient, HttpRequest, HttpResponse};
use agent_protocol::BasicInfoV1;
use agent_update::{
    HttpRequest as UpdateRequest, HttpResponse as UpdateResponse, Platform, UpdateError,
    UpdateHttp, UpdateOptions, Updater,
};
use async_trait::async_trait;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct FixedProvider;

impl ReportProvider for FixedProvider {
    fn collect_report(&self) -> ReportSample {
        ReportSample::default()
    }

    fn collect_basic_info(&self) -> BasicInfoV1 {
        BasicInfoV1 {
            cpu_name: "Mock".to_owned(),
            version: "test".to_owned(),
            ..BasicInfoV1::default()
        }
    }
}

struct FailWs {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl WsTransport for FailWs {
    async fn connect(&self, _endpoint: &str) -> Result<WsConnection, WsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(WsError::Handshake("mock offline".to_owned()))
    }
}

struct PanicWs;

#[async_trait]
impl WsTransport for PanicWs {
    async fn connect(&self, _endpoint: &str) -> Result<WsConnection, WsError> {
        panic!("the report loop must not start when an update is installed");
    }
}

#[derive(Default)]
struct RecordingHttp {
    requests: Mutex<Vec<HttpRequest>>,
}

#[async_trait]
impl HttpClient for RecordingHttp {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, DiscoveryError> {
        self.requests.lock().unwrap().push(request);
        Ok(HttpResponse {
            status: 200,
            body: Vec::new(),
        })
    }
}

struct UpdateHttpFake {
    replies: Mutex<VecDeque<UpdateResponse>>,
}

#[async_trait]
impl UpdateHttp for UpdateHttpFake {
    async fn send(&self, _request: UpdateRequest) -> Result<UpdateResponse, UpdateError> {
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected update request"))
    }
}

fn installed_update_updater(target: std::path::PathBuf) -> Updater {
    let binary = b"\x7fELFmock".to_vec();
    let release = serde_json::to_vec(&json!([{
        "tag_name": "v9.9.9",
        "name": "v9.9.9",
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

    Updater::new(
        UpdateOptions {
            repo: "owner/repo".to_owned(),
            current_version: "1.0.0".to_owned(),
            platform: Platform::new("linux", "amd64"),
            target_path: Some(target),
            ..UpdateOptions::default()
        },
        Arc::new(UpdateHttpFake {
            replies: Mutex::new(
                vec![
                    UpdateResponse {
                        status: 200,
                        body: release,
                    },
                    UpdateResponse {
                        status: 200,
                        body: binary,
                    },
                ]
                .into(),
            ),
        }),
    )
    .unwrap()
}

fn base_config() -> Config {
    Config {
        token: "token".to_owned(),
        endpoint: "http://panel.test".to_owned(),
        interval: 0.1,
        max_retries: 0,
        reconnect_interval: 0,
        disable_auto_update: true,
        ..Config::default()
    }
}

#[tokio::test]
async fn initial_update_runs_before_the_report_loop() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("komari-agent");
    std::fs::write(&target, b"old").unwrap();

    let mut config = base_config();
    config.disable_auto_update = false;

    let deps = RuntimeDeps {
        ws: Arc::new(PanicWs),
        http: Arc::new(RecordingHttp::default()),
        clock: Arc::new(TokioClock) as Arc<dyn Clock>,
        provider: Arc::new(FixedProvider),
        updater: Some(Arc::new(installed_update_updater(target))),
    };

    let exit = run_with(config, deps).await.unwrap();
    assert_eq!(exit, RunExit::Restart(42));
}

#[tokio::test]
async fn basic_info_upload_receives_cf_access_headers() {
    let mut config = base_config();
    config.cf_access_client_id = "client-id".to_owned();
    config.cf_access_client_secret = "client-secret".to_owned();

    let http = Arc::new(RecordingHttp::default());
    let ws_calls = Arc::new(AtomicUsize::new(0));
    let deps = RuntimeDeps {
        ws: Arc::new(FailWs {
            calls: ws_calls.clone(),
        }),
        http: http.clone(),
        clock: Arc::new(TokioClock) as Arc<dyn Clock>,
        provider: Arc::new(FixedProvider),
        updater: None,
    };

    let exit = run_with(config, deps).await.unwrap();
    assert_eq!(exit, RunExit::RetriesExhausted);
    assert_eq!(ws_calls.load(Ordering::SeqCst), 1);

    let requests = http.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "initial upload must happen once");
    for header in ["CF-Access-Client-Id", "CF-Access-Client-Secret"] {
        assert!(
            requests[0].headers.iter().any(|(name, _)| name == header),
            "missing {header}: {:?}",
            requests[0].headers
        );
    }
}
