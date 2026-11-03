//! The v1 report loop: connect, send a frame every interval, heartbeat, drain
//! incoming frames and reconnect according to the Go retry policy.
//!
//! Only protocol v1 is implemented. There is no v2 endpoint, no gzip and no
//! POST fallback; see `docs/parity-deviations.md` (D-004).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::basic_info::BasicInfoUploader;
use crate::clock::Clock;
use crate::endpoint::{EndpointError, build_ws_endpoint};
use crate::provider::ReportProvider;
use crate::report::{ReportSample, build_report};
use crate::ws::{WsConnection, WsEvent, WsReceiver, WsSender, WsTransport};

/// Heartbeat interval used by the Go agent.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Configuration for the report loop.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub endpoint: String,
    pub token: String,
    /// Raw `interval` in seconds from the configuration. The effective tick is
    /// `max(1, interval - 1)`, matching Go's accounting for the one second
    /// sampling window.
    pub interval: f64,
    /// Maximum reconnect attempts per connection loss. `n` means `n + 1`
    /// attempts, like Go's `retry <= MaxRetries` loop.
    pub max_retries: i64,
    pub reconnect_interval: Duration,
    pub heartbeat_interval: Duration,
    pub info_report_interval: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            token: String::new(),
            interval: 1.0,
            max_retries: 3,
            reconnect_interval: Duration::from_secs(5),
            heartbeat_interval: HEARTBEAT_INTERVAL,
            info_report_interval: Duration::from_secs(5 * 60),
        }
    }
}

impl ClientOptions {
    /// Effective data tick, matching `math.Max(1, interval-1)`.
    pub fn data_interval(&self) -> Duration {
        let seconds = if self.interval.is_finite() {
            self.interval - 1.0
        } else {
            1.0
        };
        Duration::from_secs_f64(seconds.max(1.0))
    }
}

/// Why [`AgentClient::run`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// All reconnect attempts failed. The CLI maps this to a non-zero exit so
    /// a service manager with `Restart=on-failure` starts the agent again.
    RetriesExhausted,
}

/// v1 reporting client.
pub struct AgentClient {
    options: ClientOptions,
    ws: Arc<dyn WsTransport>,
    uploader: Arc<BasicInfoUploader>,
    provider: Arc<dyn ReportProvider>,
    clock: Arc<dyn Clock>,
    ws_endpoint: String,
}

impl AgentClient {
    pub fn new(
        options: ClientOptions,
        ws: Arc<dyn WsTransport>,
        uploader: Arc<BasicInfoUploader>,
        provider: Arc<dyn ReportProvider>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, EndpointError> {
        let ws_endpoint = build_ws_endpoint(&options.endpoint, &options.token)?;
        Ok(Self {
            options,
            ws,
            uploader,
            provider,
            clock,
            ws_endpoint,
        })
    }

    /// Endpoint used for the WebSocket connection (for logs and tests).
    pub fn ws_endpoint(&self) -> &str {
        &self.ws_endpoint
    }

    /// Run until the retry budget is exhausted.
    ///
    /// Basic information is uploaded **before** the first connection attempt,
    /// exactly like Go's root command (`DoUploadBasicInfoWorks` is started
    /// before `EstablishWebSocketConnection`). A periodic upload task then runs
    /// independently, so a slow panel cannot block reports or heartbeats.
    pub async fn run(&self) -> RunOutcome {
        upload_basic_info(self.uploader.clone(), self.provider.clone()).await;

        let data_interval = self.options.data_interval();
        let heartbeat_interval = nonzero(self.options.heartbeat_interval, HEARTBEAT_INTERVAL);
        let info_interval = nonzero(
            self.options.info_report_interval,
            Duration::from_secs(5 * 60),
        );

        let info_task = tokio::spawn(periodic_basic_info(
            self.uploader.clone(),
            self.provider.clone(),
            info_interval,
        ));

        let mut data_tick =
            tokio::time::interval_at(tokio::time::Instant::now() + data_interval, data_interval);
        let mut heartbeat_tick = tokio::time::interval_at(
            tokio::time::Instant::now() + heartbeat_interval,
            heartbeat_interval,
        );

        let (disconnect_tx, mut disconnect_rx) = mpsc::channel::<u64>(4);
        let mut connection: Option<ActiveConnection> = None;
        let mut generation = 0u64;

        loop {
            tokio::select! {
                _ = data_tick.tick() => {
                    if connection.is_none() {
                        generation = generation.wrapping_add(1);
                        match self.connect_with_retry(&disconnect_tx, generation).await {
                            Some(active) => connection = Some(active),
                            None => {
                                error!("Max retries reached; stopping the v1 report loop");
                                info_task.abort();
                                return RunOutcome::RetriesExhausted;
                            }
                        }
                    }

                    let Some(sample) = collect_report(self.provider.clone()).await else {
                        continue;
                    };
                    let payload = match report_json(&sample) {
                        Ok(payload) => payload,
                        Err(err) => {
                            error!(%err, "failed to encode report");
                            continue;
                        }
                    };

                    let Some(active) = connection.as_mut() else {
                        continue;
                    };

                    if let Err(err) = active.sender.send_text(&payload).await {
                        warn!(%err, "failed to send WebSocket message; reconnecting");
                        cleanup(&mut connection).await;
                    }
                }
                _ = heartbeat_tick.tick() => {
                    let Some(active) = connection.as_mut() else {
                        continue;
                    };
                    if let Err(err) = active.sender.send_ping().await {
                        warn!(%err, "failed to send heartbeat; reconnecting");
                        cleanup(&mut connection).await;
                    }
                }
                Some(received) = disconnect_rx.recv() => {
                    if disconnect_is_current(connection.as_ref(), received) {
                        info!("WebSocket disconnected");
                        cleanup(&mut connection).await;
                    } else {
                        debug!(generation = received, "ignoring stale disconnect signal");
                    }
                }
            }
        }
    }

    async fn connect_with_retry(
        &self,
        disconnect_tx: &mpsc::Sender<u64>,
        generation: u64,
    ) -> Option<ActiveConnection> {
        if self.options.max_retries < 0 {
            return None;
        }

        let mut retry = 0i64;
        loop {
            if retry > 0 {
                info!(attempt = retry, "Retrying websocket connection");
                self.clock.sleep(self.options.reconnect_interval).await;
            }

            info!(endpoint = %self.ws_endpoint, "Attempting to connect to WebSocket");
            match self.ws.connect(&self.ws_endpoint).await {
                Ok(WsConnection { sender, receiver }) => {
                    info!("WebSocket connected (protocol v1)");
                    let handle = spawn_reader(receiver, disconnect_tx.clone(), generation);
                    return Some(ActiveConnection {
                        sender,
                        reader: handle,
                        generation,
                    });
                }
                Err(err) => {
                    warn!(%err, "Failed to connect to WebSocket");
                    retry += 1;
                    if retry > self.options.max_retries {
                        // Go always sleeps after a failed attempt, including
                        // the final one, before the outer root loop retries.
                        self.clock.sleep(self.options.reconnect_interval).await;
                        return None;
                    }
                }
            }
        }
    }
}

async fn collect_report(provider: Arc<dyn ReportProvider>) -> Option<ReportSample> {
    match tokio::task::spawn_blocking(move || provider.collect_report()).await {
        Ok(sample) => Some(sample),
        Err(err) => {
            error!(%err, "report collection task failed");
            None
        }
    }
}

async fn upload_basic_info(uploader: Arc<BasicInfoUploader>, provider: Arc<dyn ReportProvider>) {
    let info = match tokio::task::spawn_blocking(move || provider.collect_basic_info()).await {
        Ok(info) => info,
        Err(err) => {
            warn!(%err, "basic info collection task failed");
            return;
        }
    };

    match uploader.upload(&info).await {
        Ok(outcome) if outcome.reduced => {
            info!("Basic info uploaded successfully (compatibility fields removed)");
        }
        Ok(_) => info!("Basic info uploaded successfully"),
        Err(err) => warn!(%err, "Error uploading basic info"),
    }
}

async fn periodic_basic_info(
    uploader: Arc<BasicInfoUploader>,
    provider: Arc<dyn ReportProvider>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    loop {
        ticker.tick().await;
        upload_basic_info(uploader.clone(), provider.clone()).await;
    }
}

fn report_json(sample: &ReportSample) -> Result<String, agent_protocol::ProtocolError> {
    build_report(sample).to_json()
}

fn nonzero(value: Duration, fallback: Duration) -> Duration {
    if value.is_zero() { fallback } else { value }
}

fn spawn_reader(
    mut receiver: Box<dyn WsReceiver>,
    disconnected: mpsc::Sender<u64>,
    generation: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = receiver.next_event().await {
            match event {
                Ok(WsEvent::Closed) => break,
                Ok(WsEvent::Message(message)) => {
                    if !message.is_empty() {
                        debug!(
                            length = message.len(),
                            "ignoring unsolicited server message"
                        );
                    }
                }
                Ok(WsEvent::Control) => {}
                Err(err) => {
                    debug!(%err, "WebSocket read error");
                    break;
                }
            }
        }
        let _ = disconnected.send(generation).await;
    })
}

struct ActiveConnection {
    sender: Box<dyn WsSender>,
    reader: JoinHandle<()>,
    generation: u64,
}

fn disconnect_is_current(connection: Option<&ActiveConnection>, generation: u64) -> bool {
    connection.is_some_and(|active| active.generation == generation)
}

async fn cleanup(connection: &mut Option<ActiveConnection>) {
    if let Some(mut active) = connection.take() {
        active.sender.close().await;
        active.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_interval_matches_go() {
        let options = |interval| ClientOptions {
            interval,
            ..ClientOptions::default()
        };

        assert_eq!(options(1.0).data_interval(), Duration::from_secs(1));
        assert_eq!(options(0.5).data_interval(), Duration::from_secs(1));
        assert_eq!(options(2.5).data_interval(), Duration::from_millis(1500));
        assert_eq!(
            options(f64::NEG_INFINITY).data_interval(),
            Duration::from_secs(1)
        );
        assert_eq!(options(f64::NAN).data_interval(), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn stale_disconnect_generations_are_ignored() {
        let active = ActiveConnection {
            sender: Box::new(NoopSender),
            reader: tokio::spawn(async {}),
            generation: 7,
        };

        assert!(disconnect_is_current(Some(&active), 7));
        assert!(!disconnect_is_current(Some(&active), 6));
        assert!(!disconnect_is_current(None, 7));
    }

    struct NoopSender;

    #[async_trait::async_trait]
    impl WsSender for NoopSender {
        async fn send_text(&mut self, _payload: &str) -> Result<(), crate::ws::WsError> {
            Ok(())
        }
        async fn send_ping(&mut self) -> Result<(), crate::ws::WsError> {
            Ok(())
        }
        async fn close(&mut self) {}
    }
}
