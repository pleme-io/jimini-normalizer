use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::OnceLock;

use figment::providers::{Env, Format, Serialized, Yaml};
use figment::Figment;
use serde::{Deserialize, Serialize};

use crate::pii::PiiConfig;

static CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Load configuration once with figment precedence:
///   defaults → config.yaml → JIMINI_* env vars → well-known env vars
///
/// Well-known env vars (`DATABASE_URL`, `NATS_URL`, etc.) are mapped into the
/// nested config structure for backward compatibility with K8s secrets.
///
/// Prefixed env vars use double-underscore for nesting:
///   `JIMINI_SERVER__PORT=9090` → `server.port = 9090`
///
/// Call this early in main. Panics if config is invalid.
///
/// **PII config is MANDATORY** — the service will refuse to start without it.
pub fn load() -> &'static AppConfig {
    CONFIG.get_or_init(|| {
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.yaml".to_string());

        let mut cfg: AppConfig = Figment::from(Serialized::defaults(AppConfig::default()))
            .merge(Yaml::file(&config_path))
            .merge(Env::prefixed("JIMINI_").split("__"))
            // Support well-known env vars injected by K8s secrets
            .merge(
                Env::raw()
                    .only(&["DATABASE_URL", "NATS_URL", "RUST_LOG", "PORT", "HOST", "PII_HASH_SALT", "SINK_ENDPOINT"])
                    .map(|key| {
                        match key.as_str() {
                            "DATABASE_URL" => "database.url".into(),
                            "NATS_URL" => "nats.url".into(),
                            "RUST_LOG" => "logging.level".into(),
                            "PORT" => "server.port".into(),
                            "HOST" => "server.host".into(),
                            "PII_HASH_SALT" => "pii.hash_salt".into(),
                            other => other.into(),
                        }
                    }),
            )
            .extract()
            .expect("failed to load configuration");

        // PII config validation — service MUST NOT run without explicit PII rules
        cfg.pii.validate_or_panic();

        // Outbox timing validation — duplicate_window must exceed sweeper stale threshold
        // to prevent boundary race conditions where a re-published message slips past dedup.
        assert!(
            cfg.outbox.duplicate_window_secs > cfg.outbox.sweeper_stale_after_secs,
            "outbox.duplicate_window_secs ({}) must be greater than outbox.sweeper_stale_after_secs ({}) — \
             otherwise the sweeper can re-publish a message after the NATS dedup window expires, \
             causing duplicate delivery",
            cfg.outbox.duplicate_window_secs,
            cfg.outbox.sweeper_stale_after_secs,
        );

        // Map SINK_ENDPOINT env var into the first sink's endpoint for K8s secret compat
        if let Ok(endpoint) = std::env::var("SINK_ENDPOINT") {
            if !endpoint.is_empty() {
                if let Some(first) = cfg.sinks.first_mut() {
                    if first.endpoint.is_empty() {
                        first.endpoint = endpoint;
                    }
                }
            }
        }

        cfg
    })
}

/// Access the already-loaded configuration. Panics if `load()` was not called.
pub fn config() -> &'static AppConfig {
    CONFIG
        .get()
        .expect("config not loaded — call config::load() first")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub nats: NatsConfig,
    pub worker: WorkerConfig,
    pub batch: BatchConfig,
    pub retry: RetryConfig,
    pub retention: RetentionConfig,
    pub logging: LoggingConfig,
    /// List of enabled provider names. Empty = all providers enabled.
    /// Use to restrict which data sources are accepted in a given environment.
    #[serde(default)]
    pub enabled_providers: Vec<String>,
    /// PII/PHI scrubbing rules. **MANDATORY** — no defaults.
    /// Service panics on startup if this section is missing or `enabled: false`.
    pub pii: PiiConfig,
    /// Dynamic config refresh settings.
    #[serde(default)]
    pub dynamic_config: DynamicConfigSettings,
    /// Outbox settings (NATS stream for sink delivery).
    #[serde(default)]
    pub outbox: OutboxConfig,
    /// Sink instances for outbox delivery (Vector, OTEL, stdout, etc.).
    #[serde(default)]
    pub sinks: Vec<SinkInstanceConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            nats: NatsConfig::default(),
            worker: WorkerConfig::default(),
            batch: BatchConfig::default(),
            retry: RetryConfig::default(),
            retention: RetentionConfig::default(),
            logging: LoggingConfig::default(),
            enabled_providers: Vec::new(), // empty = all providers
            // PII has no meaningful default — service will panic if this
            // placeholder survives to validate_or_panic().
            pii: PiiConfig {
                enabled: false,
                patient_id_strategy: crate::pii::ScrubStrategy::Hash,
                provider_fields: std::collections::HashMap::new(),
                raw_input_strategy: crate::pii::ScrubStrategy::Hash,
                hash_salt: String::new(),
                internal: crate::pii::InternalPiiPolicy {
                    allow_internal_pii: false,
                    store_patient_id_plaintext: false,
                    store_raw_for_replay: false,
                },
            },
            dynamic_config: DynamicConfigSettings::default(),
            outbox: OutboxConfig::default(),
            sinks: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.server.host, self.server.port)
            .parse()
            .expect("invalid socket address")
    }

    pub fn metrics_socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.server.host, self.server.metrics_port)
            .parse()
            .expect("invalid metrics socket address")
    }

    /// Create a test config with sensible defaults.
    /// Does not call validate_or_panic() — PII is disabled for tests.
    pub fn default_test() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub metrics_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            metrics_port: 9090,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout_secs: u64,
    pub idle_timeout_secs: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://jimini:jimini@localhost:5432/jimini".to_string(),
            max_connections: 10,
            min_connections: 2,
            connect_timeout_secs: 30,
            idle_timeout_secs: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    pub url: String,
    pub stream_name: String,
    pub error_stream_name: String,
    pub consumer_name: String,
    pub durable: bool,
    pub pull_batch_size: usize,
    pub max_ack_pending: i64,
    pub ack_wait_secs: u64,
    pub max_deliver: i64,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            url: "nats://localhost:4222".to_string(),
            stream_name: "JIMINI_ASSESSMENTS".to_string(),
            error_stream_name: "JIMINI_ERRORS".to_string(),
            consumer_name: "jimini-normalizer".to_string(),
            durable: true,
            pull_batch_size: 10,
            max_ack_pending: 1000,
            ack_wait_secs: 30,
            max_deliver: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub shutdown_timeout_secs: u64,
    pub max_concurrent: usize,
    pub health_check_interval_secs: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_secs: 30,
            max_concurrent: 10,
            health_check_interval_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    pub days: i64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self { days: 90 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Max items to accumulate before sealing a buffer for emission
    pub buffer_size: usize,
    /// Number of batches emitted before memory cleanup (shrink_to_fit)
    pub cleanup_after_batches: usize,
    /// Max concurrent NATS publishes from a single batch
    pub max_concurrent_emits: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            buffer_size: 100,
            cleanup_after_batches: 10,
            max_concurrent_emits: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Max retry attempts for transient failures
    pub max_attempts: u32,
    /// Base delay in ms for exponential backoff
    pub base_delay_ms: u64,
    /// Maximum delay cap in ms
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 100,
            max_delay_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info,jimini_normalizer=debug".to_string(),
            format: "json".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicConfigSettings {
    /// How often (in seconds) to poll the database for config changes.
    pub refresh_interval_secs: u64,
}

impl Default for DynamicConfigSettings {
    fn default() -> Self {
        Self {
            refresh_interval_secs: 30,
        }
    }
}

fn default_outbox_stream() -> String {
    "JIMINI_OUTBOX".to_string()
}

fn default_outbox_consumer() -> String {
    "jimini-outbox".to_string()
}

fn default_sweeper_interval() -> u64 {
    60 // 1 minute
}

fn default_sweeper_stale_secs() -> u64 {
    300 // 5 minutes
}

fn default_duplicate_window_secs() -> u64 {
    600 // 10 minutes — must be > sweeper_stale_after_secs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxConfig {
    /// NATS stream name for outbox messages.
    #[serde(default = "default_outbox_stream")]
    pub stream_name: String,
    /// Consumer name for outbox workers.
    #[serde(default = "default_outbox_consumer")]
    pub consumer_name: String,
    /// Names of sinks to enable. Empty = all configured sinks enabled.
    #[serde(default)]
    pub enabled_sinks: Vec<String>,
    /// Interval (seconds) between reconciliation sweeps for stale outbox entries.
    #[serde(default = "default_sweeper_interval")]
    pub sweeper_interval_secs: u64,
    /// Age (seconds) after which a pending outbox entry is considered stale
    /// and re-published to the NATS outbox stream.
    #[serde(default = "default_sweeper_stale_secs")]
    pub sweeper_stale_after_secs: u64,
    /// NATS duplicate_window (seconds) for the outbox stream. Must be strictly
    /// greater than `sweeper_stale_after_secs` to prevent boundary race conditions.
    /// Default: 600 (10 minutes).
    #[serde(default = "default_duplicate_window_secs")]
    pub duplicate_window_secs: u64,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            stream_name: default_outbox_stream(),
            consumer_name: default_outbox_consumer(),
            enabled_sinks: Vec::new(),
            sweeper_interval_secs: default_sweeper_interval(),
            sweeper_stale_after_secs: default_sweeper_stale_secs(),
            duplicate_window_secs: default_duplicate_window_secs(),
        }
    }
}

fn default_sink_type() -> String {
    "http_json".to_string()
}

fn default_request_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkInstanceConfig {
    /// Unique name for this sink instance.
    pub name: String,
    /// Sink type: "http_json" (Vector/OTEL) or "stdout" (debug).
    #[serde(default = "default_sink_type")]
    pub sink_type: String,
    /// HTTP endpoint (for http_json sinks).
    #[serde(default)]
    pub endpoint: String,
    /// HTTP request timeout in seconds.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// Optional HTTP headers.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// Keys that are allowed to be overridden via dynamic config.
/// Startup-only (server, database, nats) and compliance-critical (pii) fields are excluded.
const ALLOWED_DYNAMIC_KEYS: &[&str] = &[
    "enabled_providers",
    "retry.max_attempts",
    "retry.base_delay_ms",
    "retry.max_delay_ms",
    "batch.buffer_size",
    "batch.max_concurrent_emits",
    "retention.days",
    "dynamic_config.refresh_interval_secs",
    "outbox.enabled_sinks",
];

impl AppConfig {
    /// Create a new config with dynamic overrides applied on top of the base config.
    ///
    /// Only keys in `ALLOWED_DYNAMIC_KEYS` are applied. Unknown keys, invalid types,
    /// and excluded keys (server.*, database.*, nats.*, pii.*) are silently ignored.
    pub fn with_dynamic_overrides(&self, overrides: &HashMap<String, serde_json::Value>) -> Self {
        let mut cfg = self.clone();

        for (key, value) in overrides {
            if !ALLOWED_DYNAMIC_KEYS.contains(&key.as_str()) {
                continue;
            }

            match key.as_str() {
                "enabled_providers" => {
                    if let Ok(providers) = serde_json::from_value::<Vec<String>>(value.clone()) {
                        cfg.enabled_providers = providers;
                    }
                }
                "retry.max_attempts" => {
                    if let Some(v) = value.as_u64() {
                        cfg.retry.max_attempts = v as u32;
                    }
                }
                "retry.base_delay_ms" => {
                    if let Some(v) = value.as_u64() {
                        cfg.retry.base_delay_ms = v;
                    }
                }
                "retry.max_delay_ms" => {
                    if let Some(v) = value.as_u64() {
                        cfg.retry.max_delay_ms = v;
                    }
                }
                "batch.buffer_size" => {
                    if let Some(v) = value.as_u64() {
                        cfg.batch.buffer_size = v as usize;
                    }
                }
                "batch.max_concurrent_emits" => {
                    if let Some(v) = value.as_u64() {
                        cfg.batch.max_concurrent_emits = v as usize;
                    }
                }
                "retention.days" => {
                    if let Some(v) = value.as_i64() {
                        cfg.retention.days = v;
                    }
                }
                "dynamic_config.refresh_interval_secs" => {
                    if let Some(v) = value.as_u64() {
                        cfg.dynamic_config.refresh_interval_secs = v;
                    }
                }
                "outbox.enabled_sinks" => {
                    if let Ok(sinks) = serde_json::from_value::<Vec<String>>(value.clone()) {
                        cfg.outbox.enabled_sinks = sinks;
                    }
                }
                _ => {}
            }
        }

        cfg
    }
}
