//! Runtime assembly: discovery, dependency construction and the report loop.
//!
//! Everything that touches the real system is built here. The individual
//! pieces are injectable ([`RuntimeDeps`]) so tests can drive the full CLI
//! wiring with mock transports.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client::{
    AgentClient, BasicInfoUploader, ClientOptions, Clock, MonitorReportProvider, ReportProvider,
    SupplementOptions, SystemSupplement, TokioClock, TungsteniteTransport, WsClientConfig,
    WsTransport,
};
use agent_config::Config;
use agent_discovery::{
    AutoDiscovery, AutoDiscoveryOptions, DnsResolver, HttpClient, PreferIp, ReqwestHttpClient,
    ResolverConfig, build_reqwest_client,
};
use agent_monitor::{Collector, MonitorOptions};
use agent_update::{CHECK_INTERVAL, ReqwestUpdateHttp, UpdateHttp, UpdateOptions, Updater};
use thiserror::Error;

use crate::VERSION;

/// Runtime failures that reach the top-level CLI.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(
        "missing credentials: set --token/--endpoint, environment variables or --auto-discovery"
    )]
    MissingCredentials,

    #[error("auto-discovery failed: {0}")]
    Discovery(#[from] agent_discovery::DiscoveryError),

    #[error("DNS configuration error: {0}")]
    Dns(#[from] agent_discovery::DnsError),

    #[error("failed to build HTTP client: {0}")]
    Http(String),

    #[error("invalid endpoint: {0}")]
    Endpoint(#[from] agent_client::EndpointError),
}

/// Where the process should go after the runtime returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunExit {
    Finished,
    Restart(i32),
}

/// Injected runtime pieces.
pub struct RuntimeDeps {
    pub ws: Arc<dyn WsTransport>,
    pub http: Arc<dyn HttpClient>,
    pub clock: Arc<dyn Clock>,
    pub provider: Arc<dyn ReportProvider>,
    /// `None` disables automatic updates (short circuit).
    pub updater: Option<Arc<Updater>>,
}

impl RuntimeDeps {
    /// Build the production dependencies from a resolved configuration.
    pub fn production(config: &Config) -> Result<Self, RuntimeError> {
        let resolver = resolver(config)?;

        let http: Arc<dyn HttpClient> = Arc::new(
            ReqwestHttpClient::new(
                resolver.clone(),
                Duration::from_secs(30),
                config.ignore_unsafe_cert,
            )
            .map_err(|err| RuntimeError::Http(err.to_string()))?,
        );

        let ws_config = WsClientConfig {
            resolver: resolver.clone(),
            handshake_timeout: Duration::from_secs(15),
            ignore_unsafe_cert: config.ignore_unsafe_cert,
            cf_access_client_id: config.cf_access_client_id.clone(),
            cf_access_client_secret: config.cf_access_client_secret.clone(),
        };
        let ws: Arc<dyn WsTransport> = Arc::new(TungsteniteTransport::new(ws_config));

        let monitor = monitor_options(config);
        let supplement = SystemSupplement::new(supplement_options(config, &monitor));
        let provider: Arc<dyn ReportProvider> = Arc::new(MonitorReportProvider::new(
            Arc::new(Collector::new(monitor)),
            supplement,
            Duration::from_secs(1),
            VERSION,
        ));

        let updater = if config.disable_auto_update {
            None
        } else {
            let client =
                build_reqwest_client(resolver, Duration::from_secs(60), config.ignore_unsafe_cert)
                    .map_err(|err| RuntimeError::Http(err.to_string()))?;
            let http: Arc<dyn UpdateHttp> = Arc::new(ReqwestUpdateHttp::with_client(client));
            let updater = Updater::new(
                UpdateOptions {
                    current_version: VERSION.to_owned(),
                    target_path: std::env::current_exe().ok(),
                    ..UpdateOptions::default()
                },
                http,
            )
            .map_err(|err| RuntimeError::Http(err.to_string()))?;
            Some(Arc::new(updater))
        };

        Ok(Self {
            ws,
            http,
            clock: Arc::new(TokioClock),
            provider,
            updater,
        })
    }
}

/// Resolve credentials from `auto-discovery.json` or register a new client.
pub async fn resolve_config(
    mut config: Config,
    http: Option<Arc<dyn HttpClient>>,
    config_path: Option<PathBuf>,
) -> Result<Config, RuntimeError> {
    if config.auto_discovery_key.is_empty() {
        if config.token.is_empty() || config.endpoint.is_empty() {
            return Err(RuntimeError::MissingCredentials);
        }
        return Ok(config);
    }

    if config.endpoint.is_empty() {
        return Err(RuntimeError::MissingCredentials);
    }

    let options = AutoDiscoveryOptions {
        endpoint: config.endpoint.clone(),
        auto_discovery_key: config.auto_discovery_key.clone(),
        config_path,
        cf_access_client_id: config.cf_access_client_id.clone(),
        cf_access_client_secret: config.cf_access_client_secret.clone(),
        timeout: Duration::from_secs(30),
        ignore_unsafe_cert: config.ignore_unsafe_cert,
        prefer_ip: prefer_ip(&config)?,
        custom_dns: non_empty(&config.custom_dns),
        http,
        ..AutoDiscoveryOptions::default()
    };

    let discovery = AutoDiscovery::new(options)?;
    let result = discovery.handle().await?;
    config.token = result.config.token;
    config.endpoint = result.endpoint;

    if config.token.is_empty() {
        return Err(RuntimeError::MissingCredentials);
    }

    Ok(config)
}

/// Run the client and the optional update loop until one of them returns.
pub async fn run_with(config: Config, deps: RuntimeDeps) -> Result<RunExit, RuntimeError> {
    let options = client_options(&config);
    let uploader = Arc::new(BasicInfoUploader::new(
        deps.http,
        config.endpoint.clone(),
        config.token.clone(),
    ));
    let client = AgentClient::new(options, deps.ws, uploader, deps.provider, deps.clock)?;

    match deps.updater {
        Some(updater) => {
            tokio::select! {
                _ = client.run() => Ok(RunExit::Finished),
                code = run_update_loop(&updater) => match code {
                    Some(code) => Ok(RunExit::Restart(code)),
                    // No restart requested (for example the update loop was
                    // cancelled or an unexpected error occurred): keep the
                    // reporting loop alive.
                    None => {
                        let _ = client.run().await;
                        Ok(RunExit::Finished)
                    }
                },
            }
        }
        None => {
            let _ = client.run().await;
            Ok(RunExit::Finished)
        }
    }
}

/// Drive `agent-update`'s periodic loop and translate the outcome to an exit
/// code. Kept public so the CLI tests can exercise the same path as `main`.
pub async fn run_update_loop(updater: &Updater) -> Option<i32> {
    match updater.run_periodic(CHECK_INTERVAL, || false).await {
        Ok(Some(outcome)) => outcome.exit_code(),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(%err, "automatic update failed");
            None
        }
    }
}

/// Map configuration to the client loop options.
pub fn client_options(config: &Config) -> ClientOptions {
    ClientOptions {
        endpoint: config.endpoint.clone(),
        token: config.token.clone(),
        interval: config.interval,
        max_retries: config.max_retries,
        reconnect_interval: Duration::from_secs(config.reconnect_interval.max(0) as u64),
        heartbeat_interval: agent_client::HEARTBEAT_INTERVAL,
        info_report_interval: Duration::from_secs((config.info_report_interval.max(1) as u64) * 60),
    }
}

/// Map configuration to monitor options, including `HOST_PROC`/`HOST_SYS` and
/// the memory accounting flags.
pub fn monitor_options(config: &Config) -> MonitorOptions {
    let mut options = MonitorOptions::from_env_lookup(&|key| std::env::var(key).ok());
    if !config.host_proc.trim().is_empty() {
        options = options.with_proc_root(config.host_proc.trim());
    }
    options.memory_include_cache = config.memory_include_cache;
    options.memory_report_raw_used = config.memory_report_raw_used;
    options
}

/// Map configuration to supplemental metrics options.
pub fn supplement_options(config: &Config, monitor: &MonitorOptions) -> SupplementOptions {
    SupplementOptions {
        proc_root: monitor.proc_root.clone(),
        sys_root: monitor.sys_root.clone(),
        include_nics: config.include_nics.clone(),
        exclude_nics: config.exclude_nics.clone(),
        include_mountpoints: config.include_mountpoints.clone(),
        enable_gpu: config.enable_gpu,
        custom_ipv4: config.custom_ipv4.clone(),
        custom_ipv6: config.custom_ipv6.clone(),
        get_ip_addr_from_nic: config.get_ip_addr_from_nic,
    }
}

/// Build the DNS resolver shared by HTTP, discovery and WebSocket.
pub fn resolver(config: &Config) -> Result<DnsResolver, RuntimeError> {
    Ok(DnsResolver::new(ResolverConfig {
        custom_dns: non_empty(&config.custom_dns),
        prefer_ip: prefer_ip(config)?,
        ..ResolverConfig::default()
    })?)
}

fn prefer_ip(config: &Config) -> Result<PreferIp, RuntimeError> {
    Ok(PreferIp::parse(&config.prefer_ip_version)?)
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            token: "tok".to_owned(),
            endpoint: "https://panel.example".to_owned(),
            ..Config::default()
        }
    }

    #[test]
    fn client_options_clamp_intervals() {
        let mut config = config();
        config.info_report_interval = 0;
        config.reconnect_interval = -5;
        let options = client_options(&config);

        assert_eq!(options.reconnect_interval, Duration::ZERO);
        assert_eq!(options.info_report_interval, Duration::from_secs(60));
        assert_eq!(options.max_retries, 3);
    }

    #[test]
    fn supplement_options_carry_filters_and_gpu_flag() {
        let mut config = config();
        config.include_nics = "eth*".to_owned();
        config.exclude_nics = "lo".to_owned();
        config.enable_gpu = true;
        let monitor = MonitorOptions::default();
        let supplement = supplement_options(&config, &monitor);

        assert_eq!(supplement.include_nics, "eth*");
        assert_eq!(supplement.exclude_nics, "lo");
        assert!(supplement.enable_gpu);
    }

    #[tokio::test]
    async fn missing_credentials_are_rejected_before_any_request() {
        let error = resolve_config(Config::default(), None, None)
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeError::MissingCredentials));
    }
}
