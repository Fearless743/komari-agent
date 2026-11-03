//! Tests for the production `reqwest` update transport.
//!
//! A tiny local HTTP/1.1 server covers redirect following, timeouts and
//! streaming size limits without any external network access.

use std::net::SocketAddr;
use std::time::Duration;

use agent_update::{HttpRequest, ReqwestUpdateHttp, UpdateError, UpdateHttp};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn start_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(handle(socket));
        }
    });

    address
}

async fn handle(mut socket: TcpStream) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 512];

    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let Ok(read) = socket.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }

    let request = String::from_utf8_lossy(&buffer);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    match path {
        "/redirect" => {
            let response = "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nslow")
                .await;
        }
        "/large" => {
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(b"x").await;
        }
        "/chunked" => {
            let response = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n5\r\nfghij\r\n0\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
        }
        _ => {
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nasset",
                )
                .await;
        }
    }

    let _ = socket.flush().await;
    let _ = socket.shutdown().await;
}

fn request(url: String, max_body_bytes: Option<u64>) -> HttpRequest {
    HttpRequest {
        url,
        headers: Vec::new(),
        max_body_bytes,
    }
}

#[tokio::test]
async fn follows_redirects() {
    let address = start_server().await;
    let http = ReqwestUpdateHttp::new(Duration::from_secs(2), false).unwrap();

    let response = http
        .send(request(format!("http://{address}/redirect"), None))
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"asset");
}

#[tokio::test]
async fn classifies_request_timeout() {
    let address = start_server().await;
    let http = ReqwestUpdateHttp::new(Duration::from_millis(100), false).unwrap();

    let error = http
        .send(request(format!("http://{address}/slow"), None))
        .await
        .unwrap_err();

    assert!(matches!(error, UpdateError::Timeout), "{error:?}");
}

#[tokio::test]
async fn rejects_large_content_length_before_reading_body() {
    let address = start_server().await;
    let http = ReqwestUpdateHttp::new(Duration::from_secs(2), false).unwrap();

    let error = http
        .send(request(format!("http://{address}/large"), Some(10)))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            UpdateError::TooLarge {
                limit: 10,
                actual: 1000
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn enforces_limit_while_streaming_chunked_body() {
    let address = start_server().await;
    let http = ReqwestUpdateHttp::new(Duration::from_secs(2), false).unwrap();

    let error = http
        .send(request(format!("http://{address}/chunked"), Some(7)))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            UpdateError::TooLarge {
                limit: 7,
                actual: 10
            }
        ),
        "{error:?}"
    );
}
