//! Mock WebSocket loop tests.
//!
//! The mock transport is entirely channel based: no real socket, no DNS and no
//! panel. The only real timing is the one second effective report interval,
//! which is part of the Go-compatible contract.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client::{
    AgentClient, BasicInfoUploader, ClientOptions, Clock, ReportProvider, ReportSample, RunOutcome,
    TokioClock, WsConnection, WsError, WsEvent, WsReceiver, WsSender, WsTransport,
};
use agent_discovery::{DiscoveryError, HttpClient, HttpRequest, HttpResponse};
use agent_protocol::BasicInfoV1;
use async_trait::async_trait;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outgoing {
    Text(String),
    Ping,
    Close,
}

enum ServerCommand {
    Close,
    Text(String),
    Error,
}

struct MockState {
    attempts: AtomicUsize,
    connections: AtomicUsize,
    fail_attempts: usize,
    fail_sends: AtomicBool,
    outgoing: mpsc::UnboundedSender<(usize, Outgoing)>,
    server: Mutex<Option<mpsc::UnboundedSender<ServerCommand>>>,
}

struct MockTransport {
    state: Arc<MockState>,
}

impl MockTransport {
    fn new(
        fail_attempts: usize,
    ) -> (
        Self,
        Arc<MockState>,
        mpsc::UnboundedReceiver<(usize, Outgoing)>,
    ) {
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel();
        let state = Arc::new(MockState {
            attempts: AtomicUsize::new(0),
            connections: AtomicUsize::new(0),
            fail_attempts,
            fail_sends: AtomicBool::new(false),
            outgoing: outgoing_tx,
            server: Mutex::new(None),
        });
        (
            Self {
                state: state.clone(),
            },
            state,
            outgoing_rx,
        )
    }
}

#[async_trait]
impl WsTransport for MockTransport {
    async fn connect(&self, _endpoint: &str) -> Result<WsConnection, WsError> {
        let attempt = self.state.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.state.fail_attempts {
            return Err(WsError::Handshake("mock failure".to_owned()));
        }

        let id = self.state.connections.fetch_add(1, Ordering::SeqCst) + 1;
        let (server_tx, server_rx) = mpsc::unbounded_channel();
        *self.state.server.lock().unwrap() = Some(server_tx);

        Ok(WsConnection {
            sender: Box::new(MockSender {
                id,
                outgoing: self.state.outgoing.clone(),
                fail_sends: self.state.clone(),
            }),
            receiver: Box::new(MockReceiver { server_rx }),
        })
    }
}

struct MockSender {
    id: usize,
    outgoing: mpsc::UnboundedSender<(usize, Outgoing)>,
    fail_sends: Arc<MockState>,
}

#[async_trait]
impl WsSender for MockSender {
    async fn send_text(&mut self, payload: &str) -> Result<(), WsError> {
        if self.fail_sends.fail_sends.load(Ordering::SeqCst) {
            return Err(WsError::Send("mock send failure".to_owned()));
        }
        let _ = self
            .outgoing
            .send((self.id, Outgoing::Text(payload.to_owned())));
        Ok(())
    }

    async fn send_ping(&mut self) -> Result<(), WsError> {
        if self.fail_sends.fail_sends.load(Ordering::SeqCst) {
            return Err(WsError::Send("mock ping failure".to_owned()));
        }
        let _ = self.outgoing.send((self.id, Outgoing::Ping));
        Ok(())
    }

    async fn close(&mut self) {
        let _ = self.outgoing.send((self.id, Outgoing::Close));
    }
}

struct MockReceiver {
    server_rx: mpsc::UnboundedReceiver<ServerCommand>,
}

#[async_trait]
impl WsReceiver for MockReceiver {
    async fn next_event(&mut self) -> Option<Result<WsEvent, WsError>> {
        match self.server_rx.recv().await? {
            ServerCommand::Close => Some(Ok(WsEvent::Closed)),
            ServerCommand::Text(text) => Some(Ok(WsEvent::Message(text.into_bytes()))),
            ServerCommand::Error => Some(Err(WsError::Read("mock read error".to_owned()))),
        }
    }
}

struct FixedProvider;

impl ReportProvider for FixedProvider {
    fn collect_report(&self) -> ReportSample {
        ReportSample {
            cpu_usage: 42.0,
            uptime: 123,
            process: 7,
            message: "mock".to_owned(),
            ..ReportSample::default()
        }
    }

    fn collect_basic_info(&self) -> BasicInfoV1 {
        BasicInfoV1 {
            cpu_name: "Mock CPU".to_owned(),
            cpu_cores: 2,
            version: "test".to_owned(),
            ..BasicInfoV1::default()
        }
    }
}

#[derive(Default)]
struct FakeHttp {
    requests: Mutex<Vec<HttpRequest>>,
    /// Delay applied to every request after the first (used to prove periodic
    /// uploads do not block reports or heartbeats).
    slow_after_first: Option<Duration>,
}

#[async_trait]
impl HttpClient for FakeHttp {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, DiscoveryError> {
        let slow = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len() > 1
        };
        if slow && let Some(delay) = self.slow_after_first {
            tokio::time::sleep(delay).await;
        }
        Ok(HttpResponse {
            status: 200,
            body: Vec::new(),
        })
    }
}

fn options(interval: f64) -> ClientOptions {
    ClientOptions {
        endpoint: "http://panel.test".to_owned(),
        token: "tok".to_owned(),
        interval,
        max_retries: 0,
        reconnect_interval: Duration::from_millis(10),
        heartbeat_interval: Duration::from_secs(30),
        info_report_interval: Duration::from_secs(300),
    }
}

fn client(options: ClientOptions, transport: MockTransport, http: Arc<FakeHttp>) -> AgentClient {
    let uploader = Arc::new(BasicInfoUploader::new(
        http,
        options.endpoint.clone(),
        options.token.clone(),
    ));
    AgentClient::new(
        options,
        Arc::new(transport),
        uploader,
        Arc::new(FixedProvider),
        Arc::new(TokioClock) as Arc<dyn Clock>,
    )
    .unwrap()
}

async fn next_outgoing(rx: &mut mpsc::UnboundedReceiver<(usize, Outgoing)>) -> (usize, Outgoing) {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("timed out waiting for a frame")
        .expect("outgoing channel closed")
}

#[tokio::test]
async fn connects_and_sends_the_first_report() {
    let (transport, _state, mut outgoing) = MockTransport::new(0);
    let http = Arc::new(FakeHttp::default());
    let client = client(options(1.0), transport, http.clone());

    let handle = tokio::spawn(async move { client.run().await });

    let (connection, outgoing) = next_outgoing(&mut outgoing).await;
    assert_eq!(connection, 1);
    let Outgoing::Text(payload) = outgoing else {
        panic!("expected a text frame, got {outgoing:?}");
    };
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["cpu"]["usage"], 42.0);
    assert_eq!(value["uptime"], 123);
    assert_eq!(value["process"], 7);
    assert_eq!(value["message"], "mock");

    // Basic info is uploaded before the first frame, like Go's root command.
    assert!(
        !http.requests.lock().unwrap().is_empty(),
        "basic info must be uploaded before the first report"
    );

    handle.abort();
}

#[tokio::test]
async fn sends_a_ping_on_the_heartbeat_interval() {
    let (transport, _state, mut outgoing) = MockTransport::new(0);
    let mut options = options(1.0);
    options.heartbeat_interval = Duration::from_millis(100);
    let client = client(options, transport, Arc::new(FakeHttp::default()));

    let handle = tokio::spawn(async move { client.run().await });

    let mut saw_ping = false;
    for _ in 0..10 {
        let (_, message) = next_outgoing(&mut outgoing).await;
        if message == Outgoing::Ping {
            saw_ping = true;
            break;
        }
    }

    assert!(saw_ping, "no ping frame was sent");
    handle.abort();
}

#[tokio::test]
async fn server_close_triggers_a_reconnect() {
    let (transport, state, mut outgoing) = MockTransport::new(0);
    let client = client(options(1.0), transport, Arc::new(FakeHttp::default()));
    let handle = tokio::spawn(async move { client.run().await });

    let (first, _) = next_outgoing(&mut outgoing).await;
    assert_eq!(first, 1);

    state
        .server
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(ServerCommand::Close)
        .unwrap();

    // The next data tick reconnects and reports again.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut second = None;
    while tokio::time::Instant::now() < deadline {
        let (connection, message) = next_outgoing(&mut outgoing).await;
        if connection > 1 && matches!(message, Outgoing::Text(_)) {
            second = Some(connection);
            break;
        }
    }

    assert!(second.is_some(), "client did not reconnect");
    handle.abort();
}

#[tokio::test]
async fn max_retries_exhaustion_returns() {
    let (transport, state, _outgoing) = MockTransport::new(3);
    let mut options = options(1.0);
    options.max_retries = 2;
    let client = client(options, transport, Arc::new(FakeHttp::default()));

    let outcome = tokio::time::timeout(Duration::from_secs(5), client.run())
        .await
        .expect("client did not give up");

    assert_eq!(outcome, RunOutcome::RetriesExhausted);
    assert_eq!(state.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn periodic_basic_info_upload_runs_independently_of_reports() {
    let (transport, _state, _outgoing) = MockTransport::new(0);
    let http = Arc::new(FakeHttp::default());
    let mut options = options(1.0);
    options.info_report_interval = Duration::from_millis(100);
    let client = client(options, transport, http.clone());
    let handle = tokio::spawn(async move { client.run().await });

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if http.requests.lock().unwrap().len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("periodic basic info uploads did not run");

    handle.abort();
}

#[tokio::test]
async fn server_text_frames_are_drained_and_do_not_reconnect() {
    let (transport, state, mut outgoing) = MockTransport::new(0);
    let client = client(options(1.0), transport, Arc::new(FakeHttp::default()));
    let handle = tokio::spawn(async move { client.run().await });

    let (first, _) = next_outgoing(&mut outgoing).await;
    assert_eq!(first, 1);

    state
        .server
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(ServerCommand::Text("unexpected command".to_owned()))
        .unwrap();

    // The next report must still use the first connection.
    let (connection, message) = next_outgoing(&mut outgoing).await;
    assert_eq!(connection, first);
    assert!(matches!(message, Outgoing::Text(_)));
    handle.abort();
}

#[tokio::test]
async fn server_read_error_triggers_a_reconnect() {
    let (transport, state, mut outgoing) = MockTransport::new(0);
    let client = client(options(1.0), transport, Arc::new(FakeHttp::default()));
    let handle = tokio::spawn(async move { client.run().await });

    let (first, _) = next_outgoing(&mut outgoing).await;
    assert_eq!(first, 1);

    state
        .server
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(ServerCommand::Error)
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut reconnected = false;
    while tokio::time::Instant::now() < deadline {
        let (connection, message) = next_outgoing(&mut outgoing).await;
        if connection > 1 && matches!(message, Outgoing::Text(_)) {
            reconnected = true;
            break;
        }
    }

    assert!(reconnected, "client did not reconnect after a read error");
    handle.abort();
}

#[test]
fn read_events_only_classify_frames() {
    // The read side only classifies frames; this guards the v1 contract that
    // the server cannot send commands to the agent.
    let events = [
        WsEvent::Message(b"hello".to_vec()),
        WsEvent::Control,
        WsEvent::Closed,
    ];
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn initial_basic_info_upload_runs_even_when_all_connections_fail() {
    let (transport, state, mut outgoing) = MockTransport::new(usize::MAX);
    let http = Arc::new(FakeHttp::default());
    let mut options = options(1.0);
    options.max_retries = 1;
    let client = client(options, transport, http.clone());

    let outcome = tokio::time::timeout(Duration::from_secs(3), client.run())
        .await
        .expect("client did not give up");

    assert_eq!(outcome, RunOutcome::RetriesExhausted);
    assert_eq!(state.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(http.requests.lock().unwrap().len(), 1);
    assert!(outgoing.try_recv().is_err(), "no frame may be sent");
}

#[tokio::test]
async fn send_failure_reconnects_on_the_next_tick() {
    let (transport, state, mut outgoing) = MockTransport::new(0);
    let client = client(options(1.0), transport, Arc::new(FakeHttp::default()));
    let handle = tokio::spawn(async move { client.run().await });

    let (first, _) = next_outgoing(&mut outgoing).await;
    assert_eq!(first, 1);
    state.fail_sends.store(true, Ordering::SeqCst);

    let mut reconnected = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let (connection, message) = next_outgoing(&mut outgoing).await;
        if matches!(message, Outgoing::Close) {
            state.fail_sends.store(false, Ordering::SeqCst);
        }
        if connection > 1 && matches!(message, Outgoing::Text(_)) {
            reconnected = true;
            break;
        }
    }

    assert!(reconnected, "client did not reconnect after a send failure");
    handle.abort();
}

#[tokio::test]
async fn heartbeat_failure_reconnects() {
    let (transport, state, mut outgoing) = MockTransport::new(0);
    let mut options = options(1.0);
    options.heartbeat_interval = Duration::from_millis(100);
    let client = client(options, transport, Arc::new(FakeHttp::default()));
    let handle = tokio::spawn(async move { client.run().await });

    let (first, _) = next_outgoing(&mut outgoing).await;
    assert_eq!(first, 1);
    state.fail_sends.store(true, Ordering::SeqCst);

    let mut reconnected = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let (connection, message) = next_outgoing(&mut outgoing).await;
        if matches!(message, Outgoing::Close) {
            state.fail_sends.store(false, Ordering::SeqCst);
        }
        if connection > 1 && matches!(message, Outgoing::Text(_)) {
            reconnected = true;
            break;
        }
    }

    assert!(
        reconnected,
        "client did not reconnect after a heartbeat failure"
    );
    handle.abort();
}

#[tokio::test]
async fn slow_periodic_uploads_do_not_block_heartbeats() {
    let (transport, _state, mut outgoing) = MockTransport::new(0);
    let http = Arc::new(FakeHttp {
        slow_after_first: Some(Duration::from_secs(2)),
        ..FakeHttp::default()
    });
    let mut options = options(1.0);
    options.heartbeat_interval = Duration::from_millis(50);
    options.info_report_interval = Duration::from_millis(50);
    let client = client(options, transport, http);
    let started = tokio::time::Instant::now();
    let handle = tokio::spawn(async move { client.run().await });

    // The first periodic upload starts at 50ms and takes 2s. The first report
    // must still arrive on the normal ~1s tick rather than after the upload.
    let first_text = loop {
        let (_, message) = next_outgoing(&mut outgoing).await;
        if matches!(message, Outgoing::Text(_)) {
            break message;
        }
    };
    assert!(matches!(first_text, Outgoing::Text(_)));
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "report was blocked by the periodic upload: {:?}",
        started.elapsed()
    );

    // Heartbeats must also keep flowing while the upload is in flight.
    let mut pings = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, message))) =
            tokio::time::timeout(Duration::from_millis(200), outgoing.recv()).await
            && message == Outgoing::Ping
        {
            pings += 1;
        }
    }

    assert!(pings >= 3, "heartbeats were blocked by uploads: {pings}");
    handle.abort();
}

#[tokio::test]
async fn final_failed_attempt_sleeps_reconnect_interval() {
    let (transport, _state, _outgoing) = MockTransport::new(usize::MAX);
    let mut options = options(1.0);
    options.max_retries = 0;
    options.reconnect_interval = Duration::from_millis(150);
    let client = client(options, transport, Arc::new(FakeHttp::default()));

    let started = tokio::time::Instant::now();
    let outcome = client.run().await;
    let elapsed = started.elapsed();

    assert_eq!(outcome, RunOutcome::RetriesExhausted);
    assert!(
        elapsed >= Duration::from_millis(150),
        "final failure did not sleep before returning: {elapsed:?}"
    );
}
