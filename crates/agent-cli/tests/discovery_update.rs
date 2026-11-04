//! Update and runtime wiring tests.
//!
//! HTTP is fully injected: no test talks to a real panel or GitHub.

use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};

use agent_update::{
    HttpRequest as UpdateRequest, HttpResponse as UpdateResponse, Platform, RESTART_EXIT_CODE,
    UpdateError, UpdateHttp, UpdateOptions, Updater,
};
use async_trait::async_trait;
use serde_json::json;

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
