//! External IP lookup tests.
//!
//! Every endpoint is a local mock HTTP server; no test resolves or contacts a
//! real service. The lookup's proxy settings are cleared unless a test opts in,
//! so a developer/CI `HTTP_PROXY` cannot redirect loopback traffic.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agent_client::{
    ExternalIpConfig, ExternalIpLookup, IpFamily, IpOptions, ProxySettings, ip_addresses_with,
};
use agent_discovery::DnsResolver;

#[derive(Clone, Debug)]
struct Request {
    line: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn path(&self) -> &str {
        self.line.split_whitespace().nth(1).unwrap_or_default()
    }
}

#[derive(Clone)]
struct Response {
    status: u16,
    body: String,
    delay: Duration,
}

impl Response {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

type Responder = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

struct MockServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    fn spawn(responder: Responder) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind IPv4 mock server");
        Self::spawn_on(listener, responder)
    }

    fn spawn_v6(responder: Responder) -> Option<Self> {
        let listener = TcpListener::bind("[::1]:0").ok()?;
        Some(Self::spawn_on(listener, responder))
    }

    fn spawn_on(listener: TcpListener, responder: Responder) -> Self {
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let addr = listener.local_addr().expect("mock server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let recorded = requests.clone();
        let stopping = stop.clone();
        let handle = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let Some(request) = read_request(&mut stream) else {
                            continue;
                        };
                        let response = responder(&request);
                        recorded.lock().unwrap().push(request);
                        if !response.delay.is_zero() {
                            thread::sleep(response.delay);
                        }
                        let _ = write_response(&mut stream, &response);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            addr,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug)]
struct SocksRequest {
    atyp: u8,
    host: String,
    port: u16,
    http_line: String,
}

type SocksResponder = Arc<dyn Fn(&SocksRequest) -> Response + Send + Sync>;

/// Minimal SOCKS5 server (no-auth, CONNECT) that records the requested target
/// and then speaks HTTP/1.1 on the same connection.
struct MockSocks5 {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<SocksRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockSocks5 {
    fn spawn(responder: SocksResponder) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind SOCKS5 mock server");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let addr = listener.local_addr().expect("SOCKS5 mock address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let recorded = requests.clone();
        let stopping = stop.clone();
        let handle = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        if let Some(request) = handle_socks5(&mut stream, &responder) {
                            recorded.lock().unwrap().push(request);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            addr,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn requests(&self) -> Vec<SocksRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockSocks5 {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_exact_bytes(stream: &mut TcpStream, length: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; length];
    stream.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn handle_socks5(stream: &mut TcpStream, responder: &SocksResponder) -> Option<SocksRequest> {
    let greeting = read_exact_bytes(stream, 2)?;
    if greeting[0] != 5 {
        return None;
    }
    let methods = read_exact_bytes(stream, usize::from(greeting[1]))?;
    if !methods.contains(&0) {
        return None;
    }
    // No authentication.
    stream.write_all(&[5, 0]).ok()?;

    let header = read_exact_bytes(stream, 4)?;
    if header[0] != 5 || header[1] != 1 {
        return None;
    }
    let atyp = header[3];
    let host = match atyp {
        1 => {
            let bytes = read_exact_bytes(stream, 4)?;
            Ipv4Addr::from(<[u8; 4]>::try_from(bytes.as_slice()).ok()?).to_string()
        }
        3 => {
            let length = usize::from(read_exact_bytes(stream, 1)?[0]);
            String::from_utf8(read_exact_bytes(stream, length)?).ok()?
        }
        4 => {
            let bytes = read_exact_bytes(stream, 16)?;
            Ipv6Addr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?).to_string()
        }
        _ => return None,
    };
    let port =
        u16::from_be_bytes(<[u8; 2]>::try_from(read_exact_bytes(stream, 2)?.as_slice()).ok()?);
    // Success, IPv4 bound address 0.0.0.0:0.
    stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).ok()?;

    let http = read_request(stream)?;
    let request = SocksRequest {
        atyp,
        host,
        port,
        http_line: http.line,
    };
    let response = responder(&request);
    write_response(stream, &response).ok()?;
    Some(request)
}

fn spawn_v6_or_skip(test: &str, responder: Responder) -> Option<MockServer> {
    match MockServer::spawn_v6(responder) {
        Some(server) => Some(server),
        None => {
            eprintln!("skipping {test}: IPv6 loopback [::1] is unavailable");
            None
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 64 * 1024 {
            break;
        }
    }

    let text = String::from_utf8_lossy(&buffer);
    let mut lines = text.split("\r\n");
    let line = lines.next()?.to_owned();
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();

    Some(Request { line, headers })
}

fn write_response(stream: &mut TcpStream, response: &Response) -> std::io::Result<()> {
    let body = response.body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {} Mock\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
        response.status,
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn config(ipv4_urls: Vec<String>, ipv6_urls: Vec<String>, timeout: Duration) -> ExternalIpConfig {
    ExternalIpConfig {
        ipv4_urls,
        ipv6_urls,
        timeout,
        ..ExternalIpConfig::default()
    }
}

fn lookup(config: ExternalIpConfig) -> ExternalIpLookup {
    ExternalIpLookup::new(config, DnsResolver::system_only()).with_proxy(ProxySettings::default())
}

fn local_lookup(ipv4_urls: Vec<String>, ipv6_urls: Vec<String>) -> ExternalIpLookup {
    lookup(config(ipv4_urls, ipv6_urls, Duration::from_secs(2)))
}

#[test]
fn ipv4_endpoints_are_tried_in_order() {
    let server = MockServer::spawn(Arc::new(|request: &Request| {
        if request.path().contains("first") {
            Response::ok("no address in this body")
        } else {
            Response::ok("ip=203.0.113.10\n")
        }
    }));

    let result = local_lookup(
        vec![server.url("/first"), server.url("/second")],
        Vec::new(),
    )
    .lookup(IpFamily::V4);

    assert_eq!(result.as_deref(), Some("203.0.113.10"));
    let paths: Vec<String> = server
        .requests()
        .into_iter()
        .map(|request| request.path().to_owned())
        .collect();
    assert_eq!(paths, vec!["/first", "/second"]);
}

#[test]
fn ipv6_endpoints_are_tried_in_order() {
    let Some(server) = spawn_v6_or_skip(
        "ipv6_endpoints_are_tried_in_order",
        Arc::new(|request: &Request| {
            if request.path().contains("first") {
                Response::ok("not an address")
            } else {
                Response::ok(r#"{"ip":"2001:db8::20"}"#)
            }
        }),
    ) else {
        return;
    };

    let result = local_lookup(
        Vec::new(),
        vec![server.url("/first"), server.url("/second")],
    )
    .lookup(IpFamily::V6);

    assert_eq!(result.as_deref(), Some("2001:db8::20"));
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn timeout_moves_to_the_next_endpoint() {
    let slow = MockServer::spawn(Arc::new(|_: &Request| {
        Response::ok("ip=203.0.113.1").delayed(Duration::from_millis(600))
    }));
    let fast = MockServer::spawn(Arc::new(|_: &Request| Response::ok("ip=198.51.100.2")));

    let started = Instant::now();
    let result = lookup(config(
        vec![slow.url("/slow"), fast.url("/fast")],
        Vec::new(),
        Duration::from_millis(100),
    ))
    .lookup(IpFamily::V4);
    let elapsed = started.elapsed();

    assert_eq!(result.as_deref(), Some("198.51.100.2"));
    assert!(
        elapsed < Duration::from_millis(550),
        "timeout did not abort the first request: {elapsed:?}"
    );
    assert_eq!(fast.requests().len(), 1);
}

#[test]
fn non_success_response_body_is_still_scanned_like_go() {
    // Go never inspects `resp.StatusCode`; it reads the body and applies the
    // regex. Keep that behaviour for parity.
    let server = MockServer::spawn(Arc::new(|_: &Request| {
        Response::status(500, "ip=203.0.113.77")
    }));

    let result = local_lookup(vec![server.url("/error")], Vec::new()).lookup(IpFamily::V4);
    assert_eq!(result.as_deref(), Some("203.0.113.77"));
}

#[test]
fn all_endpoints_failing_returns_empty() {
    let server = MockServer::spawn(Arc::new(|request: &Request| {
        if request.path().contains("empty") {
            Response::ok("no address")
        } else {
            Response::status(503, "unavailable")
        }
    }));

    let result = local_lookup(
        vec![server.url("/empty"), server.url("/unavailable")],
        Vec::new(),
    )
    .lookup(IpFamily::V4);

    assert!(result.is_none());
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn external_lookup_sends_the_go_user_agent() {
    let server = MockServer::spawn(Arc::new(|_: &Request| Response::ok("ip=203.0.113.8")));
    let result = local_lookup(vec![server.url("/ua")], Vec::new()).lookup(IpFamily::V4);

    assert_eq!(result.as_deref(), Some("203.0.113.8"));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("user-agent"), Some("curl/8.0.1"));
}

#[test]
fn proxy_settings_route_the_request_through_the_proxy() {
    let proxy = MockServer::spawn(Arc::new(|_: &Request| {
        Response::ok(r#"{"ip":"198.51.100.4"}"#)
    }));

    let proxy_settings = ProxySettings {
        http: Some(format!("http://{}", proxy.addr)),
        https: None,
        no_proxy: None,
    };
    let lookup = ExternalIpLookup::new(
        config(
            vec!["http://external.invalid/ip".to_owned()],
            Vec::new(),
            Duration::from_secs(2),
        ),
        DnsResolver::system_only(),
    )
    .with_proxy(proxy_settings);

    let result = lookup.lookup(IpFamily::V4);

    assert_eq!(result.as_deref(), Some("198.51.100.4"));
    let requests = proxy.requests();
    assert_eq!(requests.len(), 1);
    // A proxied HTTP request uses the absolute form, so the target host is
    // visible even though it cannot be resolved locally.
    assert!(
        requests[0].line.contains("http://external.invalid/ip"),
        "unexpected request line: {}",
        requests[0].line
    );
}

#[test]
fn ipv4_client_rejects_an_ipv6_literal() {
    let Some(server) = spawn_v6_or_skip(
        "ipv4_client_rejects_an_ipv6_literal",
        Arc::new(|_: &Request| {
            // Contains both families so a successful IPv4 connection would be
            // visible in the assertion below.
            Response::ok("ip=203.0.113.30\n2001:db8::30")
        }),
    ) else {
        return;
    };

    let v6_url = server.url("/v6");
    let v4_result = lookup(config(
        vec![v6_url.clone()],
        Vec::new(),
        Duration::from_millis(500),
    ))
    .lookup(IpFamily::V4);
    assert!(v4_result.is_none(), "IPv4 client used an IPv6 endpoint");

    // The same server works when the IPv6 client is selected.
    let v6_result =
        lookup(config(Vec::new(), vec![v6_url], Duration::from_millis(500))).lookup(IpFamily::V6);
    assert_eq!(v6_result.as_deref(), Some("2001:db8::30"));
}

#[test]
fn ipv6_client_rejects_an_ipv4_literal() {
    let server = MockServer::spawn(Arc::new(|_: &Request| Response::ok("2001:db8::30")));

    let result = lookup(config(
        Vec::new(),
        vec![server.url("/v4")],
        Duration::from_millis(500),
    ))
    .lookup(IpFamily::V6);

    assert!(result.is_none(), "IPv6 client used an IPv4 endpoint");
    assert!(server.requests().is_empty());
}

#[test]
fn custom_addresses_win_over_external_lookup() {
    let server = MockServer::spawn(Arc::new(|_: &Request| Response::ok("203.0.113.99")));
    let options = IpOptions {
        custom_ipv4: "203.0.113.7".to_owned(),
        custom_ipv6: "2001:db8::7".to_owned(),
        external: config(
            vec![server.url("/ip")],
            vec![server.url("/ip")],
            Duration::from_secs(1),
        ),
        ..IpOptions::default()
    };
    let lookup = ExternalIpLookup::new(options.external.clone(), DnsResolver::system_only())
        .with_proxy(ProxySettings::default());

    assert_eq!(
        ip_addresses_with(&options, &lookup),
        ("203.0.113.7".to_owned(), "2001:db8::7".to_owned())
    );
    assert!(
        server.requests().is_empty(),
        "external lookup ran despite custom values"
    );
}

#[test]
fn external_lookup_fills_only_the_missing_family() {
    let Some(server) = spawn_v6_or_skip(
        "external_lookup_fills_only_the_missing_family",
        Arc::new(|_: &Request| Response::ok("2001:db8::99")),
    ) else {
        return;
    };
    let options = IpOptions {
        custom_ipv4: "203.0.113.7".to_owned(),
        custom_ipv6: String::new(),
        external: config(Vec::new(), vec![server.url("/ip")], Duration::from_secs(1)),
        ..IpOptions::default()
    };
    let lookup = ExternalIpLookup::new(options.external.clone(), DnsResolver::system_only())
        .with_proxy(ProxySettings::default());

    assert_eq!(
        ip_addresses_with(&options, &lookup),
        ("203.0.113.7".to_owned(), "2001:db8::99".to_owned())
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn socks5_proxy_is_used_with_remote_name_resolution() {
    let socks = MockSocks5::spawn(Arc::new(|_: &SocksRequest| {
        Response::ok(r#"{"ip":"198.51.100.6"}"#)
    }));
    let settings = ProxySettings {
        http: Some(format!("socks5://{}", socks.addr)),
        https: None,
        no_proxy: None,
    };
    let lookup = ExternalIpLookup::new(
        config(
            // This name cannot resolve locally; only the SOCKS proxy can
            // accept it, proving the socks5h (remote DNS) translation.
            vec!["http://external.invalid/ip".to_owned()],
            Vec::new(),
            Duration::from_secs(2),
        ),
        DnsResolver::system_only(),
    )
    .with_proxy(settings);

    assert_eq!(lookup.lookup(IpFamily::V4).as_deref(), Some("198.51.100.6"));

    let requests = socks.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].atyp, 3, "expected a domain-name SOCKS target");
    assert_eq!(requests[0].host, "external.invalid");
    assert_eq!(requests[0].port, 80);
    assert_eq!(requests[0].http_line, "GET /ip HTTP/1.1");
}

#[test]
fn socks5_scheme_is_case_insensitive() {
    let socks = MockSocks5::spawn(Arc::new(|_: &SocksRequest| Response::ok("ip=198.51.100.7")));
    let settings = ProxySettings {
        http: Some(format!("SOCKS5://{}", socks.addr)),
        https: None,
        no_proxy: None,
    };
    let lookup = ExternalIpLookup::new(
        config(
            vec!["http://external.invalid/ip".to_owned()],
            Vec::new(),
            Duration::from_secs(2),
        ),
        DnsResolver::system_only(),
    )
    .with_proxy(settings);

    assert_eq!(lookup.lookup(IpFamily::V4).as_deref(), Some("198.51.100.7"));
    assert_eq!(socks.requests().len(), 1);
}

#[test]
fn https_endpoint_uses_connect_through_an_http_proxy() {
    let proxy = MockServer::spawn(Arc::new(|request: &Request| {
        // The tunnel is deliberately rejected; the assertion only verifies
        // that HTTPS traffic is routed through CONNECT.
        assert!(request.line.starts_with("CONNECT "), "{}", request.line);
        Response::status(502, "tunnel refused")
    }));
    let settings = ProxySettings {
        http: None,
        https: Some(format!("http://{}", proxy.addr)),
        no_proxy: None,
    };
    let lookup = ExternalIpLookup::new(
        config(
            vec!["https://external.invalid/ip".to_owned()],
            Vec::new(),
            Duration::from_secs(1),
        ),
        DnsResolver::system_only(),
    )
    .with_proxy(settings);

    assert!(lookup.lookup(IpFamily::V4).is_none());

    let requests = proxy.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].line.contains("external.invalid:443"),
        "unexpected CONNECT target: {}",
        requests[0].line
    );
}

#[test]
fn no_proxy_bypasses_the_configured_proxy() {
    let origin = MockServer::spawn(Arc::new(|_: &Request| Response::ok("ip=203.0.113.44")));
    let proxy = MockServer::spawn(Arc::new(|_: &Request| Response::ok("ip=198.51.100.44")));
    let settings = ProxySettings {
        http: Some(format!("http://{}", proxy.addr)),
        https: None,
        no_proxy: Some("127.0.0.1".to_owned()),
    };
    let lookup = ExternalIpLookup::new(
        config(vec![origin.url("/ip")], Vec::new(), Duration::from_secs(1)),
        DnsResolver::system_only(),
    )
    .with_proxy(settings);

    assert_eq!(lookup.lookup(IpFamily::V4).as_deref(), Some("203.0.113.44"));
    assert_eq!(origin.requests().len(), 1);
    assert!(
        proxy.requests().is_empty(),
        "NO_PROXY host was sent to the proxy"
    );
}

#[test]
fn nic_disabled_and_no_custom_values_queries_both_families() {
    let v4 = MockServer::spawn(Arc::new(|_: &Request| Response::ok("ip=203.0.113.55")));
    let Some(v6) = spawn_v6_or_skip(
        "nic_disabled_and_no_custom_values_queries_both_families",
        Arc::new(|_: &Request| Response::ok("2001:db8::55")),
    ) else {
        return;
    };
    let options = IpOptions {
        external: config(
            vec![v4.url("/v4")],
            vec![v6.url("/v6")],
            Duration::from_secs(1),
        ),
        ..IpOptions::default()
    };
    let lookup = ExternalIpLookup::new(options.external.clone(), DnsResolver::system_only())
        .with_proxy(ProxySettings::default());

    assert_eq!(
        ip_addresses_with(&options, &lookup),
        ("203.0.113.55".to_owned(), "2001:db8::55".to_owned())
    );
    assert_eq!(v4.requests().len(), 1);
    assert_eq!(v6.requests().len(), 1);
}
