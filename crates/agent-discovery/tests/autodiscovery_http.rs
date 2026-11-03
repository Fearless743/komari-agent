//! Auto discovery tests against a hand-written local HTTP server.
//!
//! The server only implements the small subset of HTTP/1.1 used by `reqwest`;
//! it lets the tests exercise the real transport, headers, status handling and
//! timeouts without any external network access.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_discovery::{
    AutoDiscovery, AutoDiscoveryOptions, DiscoveryError, PreferIp, default_config_path,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Clone, Debug)]
struct MockReply {
    status: u16,
    body: Vec<u8>,
    delay: Duration,
}

impl MockReply {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
        }
    }

    fn delayed(status: u16, body: &str, delay: Duration) -> Self {
        Self {
            status,
            body: body.as_bytes().to_vec(),
            delay,
        }
    }
}

type Requests = Arc<Mutex<Vec<CapturedRequest>>>;

async fn start_http(reply: MockReply) -> (SocketAddr, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let reply = reply.clone();
            let captured = captured.clone();

            tokio::spawn(async move {
                if let Some(request) = read_request(&mut socket).await {
                    captured.lock().unwrap().push(request);
                    if !reply.delay.is_zero() {
                        tokio::time::sleep(reply.delay).await;
                    }
                    let _ = write_response(socket, &reply).await;
                }
            });
        }
    });

    (address, requests)
}

async fn read_request(socket: &mut TcpStream) -> Option<CapturedRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];

    let header_end = loop {
        if let Some(position) = find_subslice(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let headers_text = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = headers_text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);

    let body_start = header_end + 4;
    let mut body = buffer[body_start..].to_vec();
    while body.len() < content_length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    Some(CapturedRequest {
        method,
        path,
        headers,
        body,
    })
}

async fn write_response(mut socket: TcpStream, reply: &MockReply) -> std::io::Result<()> {
    let reason = match reply.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        500 => "Internal Server Error",
        _ => "Status",
    };

    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reason,
        reply.body.len()
    );

    socket.write_all(head.as_bytes()).await?;
    socket.write_all(&reply.body).await?;
    socket.flush().await?;
    socket.shutdown().await
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn options(endpoint: SocketAddr, config_path: std::path::PathBuf) -> AutoDiscoveryOptions {
    AutoDiscoveryOptions {
        endpoint: format!("http://{endpoint}"),
        auto_discovery_key: "secret-key".to_owned(),
        hostname: Some("test-host".to_owned()),
        config_path: Some(config_path),
        cf_access_client_id: "cf-id".to_owned(),
        cf_access_client_secret: "cf-secret".to_owned(),
        timeout: Duration::from_secs(2),
        ignore_unsafe_cert: false,
        prefer_ip: PreferIp::V4,
        custom_dns: None,
        resolver: None,
        http: None,
    }
}

#[tokio::test]
async fn registers_successfully_and_saves_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    let (address, requests) = start_http(MockReply::json(
        200,
        r#"{"status":"success","message":"","data":{"uuid":"uuid-1","token":"token-1"}}"#,
    ))
    .await;

    let discovery = AutoDiscovery::new(options(address, config_path.clone())).unwrap();
    let result = discovery.handle().await.unwrap();

    assert!(result.registered);
    assert_eq!(result.config.uuid, "uuid-1");
    assert_eq!(result.config.token, "token-1");

    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(saved["uuid"], "uuid-1");
    assert_eq!(saved["token"], "token-1");

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let request = &captured[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/api/clients/register?name=test-host");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer secret-key")
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        request
            .headers
            .get("cf-access-client-id")
            .map(String::as_str),
        Some("cf-id")
    );
    assert_eq!(
        request
            .headers
            .get("cf-access-client-secret")
            .map(String::as_str),
        Some("cf-secret")
    );

    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["key"], "secret-key");
}

#[tokio::test]
async fn reuses_existing_config_without_network_access() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    fs::write(
        &config_path,
        r#"{"uuid":"existing-uuid","token":"existing-token"}"#,
    )
    .unwrap();

    // Nothing listens on port 1; a network attempt would fail.
    let discovery = AutoDiscovery::new(AutoDiscoveryOptions {
        endpoint: "http://127.0.0.1:1".to_owned(),
        auto_discovery_key: "unused".to_owned(),
        config_path: Some(config_path),
        ..AutoDiscoveryOptions::default()
    })
    .unwrap();

    let result = discovery.handle().await.unwrap();
    assert!(!result.registered);
    assert_eq!(result.config.uuid, "existing-uuid");
    assert_eq!(result.config.token, "existing-token");
}

#[tokio::test]
async fn reports_http_401_with_body() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    let (address, _) = start_http(MockReply::json(401, "unauthorized")).await;

    let discovery = AutoDiscovery::new(options(address, config_path.clone())).unwrap();
    let error = discovery.handle().await.unwrap_err();

    match error {
        DiscoveryError::HttpStatus { status, body } => {
            assert_eq!(status, 401);
            assert_eq!(body, "unauthorized");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!config_path.exists());
}

#[tokio::test]
async fn rejects_invalid_json_without_saving_config() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    let (address, _) = start_http(MockReply::json(200, "<html>not json</html>")).await;

    let discovery = AutoDiscovery::new(options(address, config_path.clone())).unwrap();
    let error = discovery.handle().await.unwrap_err();

    assert!(matches!(error, DiscoveryError::InvalidJson(_)), "{error:?}");
    assert!(!config_path.exists());
}

#[tokio::test]
async fn rejects_non_success_status_field() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    let (address, _) = start_http(MockReply::json(
        200,
        r#"{"status":"error","message":"invalid key","data":{}}"#,
    ))
    .await;

    let discovery = AutoDiscovery::new(options(address, config_path.clone())).unwrap();
    let error = discovery.handle().await.unwrap_err();

    match error {
        DiscoveryError::Rejected(message) => assert_eq!(message, "invalid key"),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!config_path.exists());
}

#[tokio::test]
async fn times_out_when_server_does_not_answer() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    let (address, _) = start_http(MockReply::delayed(
        200,
        r#"{"status":"success","data":{"uuid":"u","token":"t"}}"#,
        Duration::from_secs(2),
    ))
    .await;

    let mut settings = options(address, config_path.clone());
    settings.timeout = Duration::from_millis(120);

    let discovery = AutoDiscovery::new(settings).unwrap();
    let error = discovery.handle().await.unwrap_err();

    assert!(matches!(error, DiscoveryError::Http(_)), "{error:?}");
    assert!(!config_path.exists());
}

#[tokio::test]
async fn corrupt_config_is_replaced_by_registration() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("auto-discovery.json");
    fs::write(&config_path, "{ broken").unwrap();

    let (address, _) = start_http(MockReply::json(
        200,
        r#"{"status":"success","data":{"uuid":"new-uuid","token":"new-token"}}"#,
    ))
    .await;

    let discovery = AutoDiscovery::new(options(address, config_path.clone())).unwrap();
    let result = discovery.handle().await.unwrap();

    assert!(result.registered);
    assert_eq!(result.config.uuid, "new-uuid");
    assert!(default_config_path().ends_with("auto-discovery.json"));
}
