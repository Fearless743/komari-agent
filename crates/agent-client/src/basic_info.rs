//! Basic information payload and upload.
//!
//! Go first posts the full payload and, when the request fails, retries once
//! after deleting `kernel_version` (panels <= 1.0.2) and `cpu_physical_cores`
//! (panels <= 1.2.0). The same fallback is implemented here. The HTTP transport
//! is the injectable [`HttpClient`] from `agent-discovery`.

use std::sync::Arc;

use agent_discovery::{HttpClient, HttpRequest, HttpResponse};
use agent_monitor::BasicInfo;
use agent_protocol::BasicInfoV1;
use thiserror::Error;

use crate::endpoint::build_basic_info_endpoint;

/// Build the v1 basic information object.
///
/// `disk_total` and `gpu_name` come from A03's [`BasicInfo`]; only the IP
/// addresses are supplied by the A05 IP-only module.
pub fn build_basic_info(info: &BasicInfo, ipv4: &str, ipv6: &str, version: &str) -> BasicInfoV1 {
    BasicInfoV1 {
        cpu_name: info.cpu_name.clone(),
        cpu_cores: i64::from(info.cpu_cores),
        cpu_physical_cores: Some(i64::from(info.cpu_physical_cores)),
        arch: info.arch.clone(),
        os: info.os.clone(),
        kernel_version: Some(info.kernel_version.clone()),
        ipv4: ipv4.to_owned(),
        ipv6: ipv6.to_owned(),
        mem_total: info.mem_total,
        swap_total: info.swap_total,
        disk_total: info.disk_total,
        gpu_name: info.gpu_name.clone(),
        virtualization: info.virtualization.clone(),
        version: version.to_owned(),
        ..BasicInfoV1::default()
    }
}

/// Upload failures. Both attempts are kept so logs show whether the panel
/// rejected the reduced payload too.
#[derive(Debug, Error)]
pub enum UploadError {
    #[error("invalid endpoint: {0}")]
    Endpoint(#[from] crate::endpoint::EndpointError),

    #[error("basic info upload failed: full payload: {full}; reduced payload: {reduced}")]
    Rejected { full: String, reduced: String },

    #[error("basic info upload failed: {0}")]
    Transport(String),
}

/// Whether the reduced payload was needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadOutcome {
    pub reduced: bool,
}

/// Basic information uploader.
pub struct BasicInfoUploader {
    http: Arc<dyn HttpClient>,
    endpoint: String,
    token: String,
    cf_access_client_id: String,
    cf_access_client_secret: String,
}

impl BasicInfoUploader {
    pub fn new(
        http: Arc<dyn HttpClient>,
        endpoint: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            http,
            endpoint: endpoint.into(),
            token: token.into(),
            cf_access_client_id: String::new(),
            cf_access_client_secret: String::new(),
        }
    }

    pub fn with_access_headers(
        mut self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.cf_access_client_id = client_id.into();
        self.cf_access_client_secret = client_secret.into();
        self
    }

    /// Upload, retrying once with the compatibility fields removed.
    pub async fn upload(&self, info: &BasicInfoV1) -> Result<UploadOutcome, UploadError> {
        match self.send(info).await {
            Ok(()) => Ok(UploadOutcome { reduced: false }),
            Err(full) => {
                let mut reduced = info.clone();
                reduced.kernel_version = None;
                reduced.cpu_physical_cores = None;

                match self.send(&reduced).await {
                    Ok(()) => Ok(UploadOutcome { reduced: true }),
                    Err(reduced_error) => Err(UploadError::Rejected {
                        full: full.to_string(),
                        reduced: reduced_error.to_string(),
                    }),
                }
            }
        }
    }

    async fn send(&self, info: &BasicInfoV1) -> Result<(), UploadError> {
        let url = build_basic_info_endpoint(&self.endpoint, &self.token)?;
        let body = info
            .to_json()
            .map_err(|err| UploadError::Transport(err.to_string()))?
            .into_bytes();

        let mut headers = vec![("Content-Type".to_owned(), "application/json".to_owned())];
        if !self.cf_access_client_id.is_empty() && !self.cf_access_client_secret.is_empty() {
            headers.push((
                "CF-Access-Client-Id".to_owned(),
                self.cf_access_client_id.clone(),
            ));
            headers.push((
                "CF-Access-Client-Secret".to_owned(),
                self.cf_access_client_secret.clone(),
            ));
        }

        let response = self
            .http
            .execute(HttpRequest {
                method: "POST".to_owned(),
                url,
                headers,
                body: Some(body),
            })
            .await
            .map_err(|err| UploadError::Transport(err.to_string()))?;

        check_response(&response)
    }
}

fn check_response(response: &HttpResponse) -> Result<(), UploadError> {
    if response.status == 200 {
        return Ok(());
    }

    Err(UploadError::Transport(format!(
        "server returned status {}: {}",
        response.status,
        String::from_utf8_lossy(&response.body)
    )))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use agent_discovery::DiscoveryError;
    use async_trait::async_trait;

    use super::*;

    #[derive(Default)]
    struct FakeHttp {
        responses: Mutex<Vec<u16>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    #[async_trait]
    impl HttpClient for FakeHttp {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, DiscoveryError> {
            self.requests.lock().unwrap().push(request);
            let status = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .or_else(|| Some(200))
                .unwrap();
            Ok(HttpResponse {
                status,
                body: Vec::new(),
            })
        }
    }

    fn sample_info() -> BasicInfoV1 {
        BasicInfoV1 {
            cpu_name: "Example CPU".to_owned(),
            cpu_cores: 4,
            cpu_physical_cores: Some(2),
            arch: "x86_64".to_owned(),
            os: "Linux".to_owned(),
            kernel_version: Some("6.1".to_owned()),
            ipv4: "192.0.2.1".to_owned(),
            ipv6: String::new(),
            mem_total: 100,
            swap_total: 20,
            disk_total: 1000,
            gpu_name: "None".to_owned(),
            virtualization: "kvm".to_owned(),
            version: "1.2.3".to_owned(),
            ..BasicInfoV1::default()
        }
    }

    #[tokio::test]
    async fn posts_the_full_payload_to_the_go_endpoint() {
        let http = Arc::new(FakeHttp::default());
        let uploader = BasicInfoUploader::new(http.clone(), "https://panel.example/", "tok");
        let outcome = uploader.upload(&sample_info()).await.unwrap();

        assert_eq!(outcome, UploadOutcome { reduced: false });
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(
            requests[0].url,
            "https://panel.example/api/clients/uploadBasicInfo?token=tok"
        );

        let body: serde_json::Value =
            serde_json::from_slice(requests[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["cpu_name"], "Example CPU");
        assert_eq!(body["cpu_cores"], 4);
        assert_eq!(body["cpu_physical_cores"], 2);
        assert_eq!(body["kernel_version"], "6.1");
        assert_eq!(body["version"], "1.2.3");
        assert_eq!(body["ipv4"], "192.0.2.1");
        assert_eq!(body["ipv6"], "");
        assert_eq!(body["disk_total"], 1000);
    }

    #[tokio::test]
    async fn cloudflare_access_headers_are_sent_on_both_attempts() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![200, 500]),
            requests: Mutex::new(Vec::new()),
        });
        let uploader = BasicInfoUploader::new(http.clone(), "https://panel.example", "tok")
            .with_access_headers("client-id", "client-secret");
        uploader.upload(&sample_info()).await.unwrap();

        let requests = http.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(name, value)| { name == "CF-Access-Client-Id" && value == "client-id" })
            );
            assert!(request.headers.iter().any(|(name, value)| {
                name == "CF-Access-Client-Secret" && value == "client-secret"
            }));
        }
    }

    #[tokio::test]
    async fn retries_without_the_legacy_fields() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![200, 500]),
            requests: Mutex::new(Vec::new()),
        });
        let uploader = BasicInfoUploader::new(http.clone(), "https://panel.example", "tok");
        let outcome = uploader.upload(&sample_info()).await.unwrap();

        assert_eq!(outcome, UploadOutcome { reduced: true });
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);

        let full: serde_json::Value =
            serde_json::from_slice(requests[0].body.as_ref().unwrap()).unwrap();
        assert!(full.get("kernel_version").is_some());
        assert!(full.get("cpu_physical_cores").is_some());

        let reduced: serde_json::Value =
            serde_json::from_slice(requests[1].body.as_ref().unwrap()).unwrap();
        assert!(reduced.get("kernel_version").is_none());
        assert!(reduced.get("cpu_physical_cores").is_none());
        assert_eq!(reduced["cpu_name"], "Example CPU");
    }

    #[tokio::test]
    async fn both_attempts_failing_reports_both_errors() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![500, 500]),
            requests: Mutex::new(Vec::new()),
        });
        let uploader = BasicInfoUploader::new(http.clone(), "https://panel.example", "tok");
        let error = uploader.upload(&sample_info()).await.unwrap_err();

        assert!(matches!(error, UploadError::Rejected { .. }));
        assert_eq!(http.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn basic_info_mapping_matches_the_go_map() {
        let info = BasicInfo {
            cpu_name: "CPU".to_owned(),
            cpu_cores: 8,
            cpu_physical_cores: 4,
            arch: "aarch64".to_owned(),
            os: "Debian".to_owned(),
            kernel_version: "6.6".to_owned(),
            mem_total: 1024,
            swap_total: 2048,
            disk_total: 4096,
            gpu_name: "None".to_owned(),
            virtualization: "none".to_owned(),
        };

        let basic = build_basic_info(&info, "192.0.2.2", "", "1.0.0");
        assert_eq!(basic.cpu_cores, 8);
        assert_eq!(basic.cpu_physical_cores, Some(4));
        assert_eq!(basic.mem_total, 1024);
        assert_eq!(basic.disk_total, 4096);
        assert_eq!(basic.version, "1.0.0");
        let value: serde_json::Value = serde_json::from_str(&basic.to_json().unwrap()).unwrap();
        assert_eq!(value["arch"], "aarch64");
        assert_eq!(value["ipv6"], "");
    }
}
