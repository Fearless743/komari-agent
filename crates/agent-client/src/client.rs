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
    /// All reconnect attempts failed. The caller decides whether to restart the
    /// loop (Go's root command starts it again) or exit.
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
    pub async fn run(&self) -> RunOutcome {
        let data_interval = self.options.data_interval();
        let heartbeat_interval = nonzero(self.options.heartbeat_interval, HEARTBEAT_INTERVAL);
        let info_interval = nonzero(
            self.options.info_report_interval,
            Duration::from_secs(5 * 60),
        );

        let mut data_tick =
            tokio::time::interval_at(tokio::time::Instant::now() + data_interval, data_interval);
        let mut heartbeat_tick = tokio::time::interval_at(
            tokio::time::Instant::now() + heartbeat_interval,
            heartbeat_interval,
        );
        let mut info_tick =
            tokio::time::interval_at(tokio::time::Instant::now() + info_interval, info_interval);

        let (disconnect_tx, mut disconnect_rx) = mpsc::channel::<()>(4);
        let mut connection: Option<ActiveConnection> = None;
        let mut uploaded_initial_info = false;

        loop {
            tokio::select! {
                _ = data_tick.tick() => {
                    if connection.is_none() {
                        match self.connect_with_retry(&disconnect_tx).await {
                            Some(active) => {
                                let first = !uploaded_initial_info;
                                connection = Some(active);
                                if first {
                                    uploaded_initial_info = true;
                                    self.upload_basic_info().await;
                                }
                            }
                            None => {
                                error!("Max retries reached; stopping the v1 report loop");
                                return RunOutcome::RetriesExhausted;
                            }
                        }
                    }

                    let sample = self.provider.collect_report();
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
                _ = info_tick.tick() => {
                    self.upload_basic_info().await;
                }
                Some(()) = disconnect_rx.recv() => {
                    info!("WebSocket disconnected");
                    cleanup(&mut connection).await;
                }
            }
        }
    }

    async fn connect_with_retry(
        &self,
        disconnect_tx: &mpsc::Sender<()>,
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
                    let handle = spawn_reader(receiver, disconnect_tx.clone());
                    return Some(ActiveConnection {
                        sender,
                        reader: handle,
                    });
                }
                Err(err) => {
                    warn!(%err, "Failed to connect to WebSocket");
                    retry += 1;
                    if retry > self.options.max_retries {
                        return None;
                    }
                }
            }
        }
    }

    async fn upload_basic_info(&self) {
        let info = self.provider.collect_basic_info();
        match self.uploader.upload(&info).await {
            Ok(outcome) if outcome.reduced => {
                info!("Basic info uploaded successfully (compatibility fields removed)");
            }
            Ok(_) => info!("Basic info uploaded successfully"),
            Err(err) => warn!(%err, "Error uploading basic info"),
        }
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
    disconnected: mpsc::Sender<()>,
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
        let _ = disconnected.send(()).await;
    })
}

struct ActiveConnection {
    sender: Box<dyn WsSender>,
    reader: JoinHandle<()>,
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
}
