pub mod http_json;
pub mod postgres;
pub mod stdout;

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::info;

use crate::config::SinkInstanceConfig;

/// Error type for sink operations.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("sink not configured: {0}")]
    NotConfigured(String),
}

/// Trait implemented by each sink adapter.
///
/// To add a new sink:
/// 1. Create `src/sink/my_sink.rs` implementing this trait
/// 2. Add `pub mod my_sink;` to this file
/// 3. Add the sink type string to `create_sink()` below
/// 4. Add a `SinkInstanceConfig` entry in config.yaml
#[async_trait]
pub trait Sink: Send + Sync {
    /// Unique name for this sink instance (from config).
    fn name(&self) -> &str;

    /// Sink type identifier (e.g. "http_json", "stdout").
    fn sink_type(&self) -> &str;

    /// Send a payload to the sink. Returns Ok(()) on success.
    async fn send(&self, payload: &serde_json::Value) -> Result<(), SinkError>;
}

/// Registry of all configured sinks, keyed by name.
///
/// Optionally filters to only the sinks listed in `enabled_sinks`.
/// If `enabled_sinks` is empty, all configured sinks are registered.
pub struct SinkRegistry {
    sinks: HashMap<String, Box<dyn Sink>>,
}

impl SinkRegistry {
    /// Create a registry with all configured sinks enabled.
    pub fn new(configs: &[SinkInstanceConfig]) -> Self {
        Self::with_filter(configs, None)
    }

    /// Create a registry filtered to only the specified sink names.
    /// If `enabled` is None or empty, all configured sinks are registered.
    pub fn with_filter(configs: &[SinkInstanceConfig], enabled: Option<&[String]>) -> Self {
        let mut registry = Self {
            sinks: HashMap::new(),
        };

        for config in configs {
            if let Some(enabled_list) = enabled {
                if !enabled_list.is_empty() && !enabled_list.iter().any(|e| e == &config.name) {
                    info!(sink = %config.name, "sink configured but not enabled — skipping");
                    continue;
                }
            }

            match create_sink(config) {
                Some(sink) => {
                    info!(
                        sink = %config.name,
                        sink_type = %config.sink_type,
                        "registered sink"
                    );
                    registry.sinks.insert(config.name.clone(), sink);
                }
                None => {
                    tracing::warn!(
                        sink = %config.name,
                        sink_type = %config.sink_type,
                        "unknown sink type — skipping"
                    );
                }
            }
        }

        registry
    }

    pub fn get(&self, name: &str) -> Option<&dyn Sink> {
        self.sinks.get(name).map(|s| s.as_ref())
    }

    pub fn all(&self) -> Vec<&dyn Sink> {
        self.sinks.values().map(|s| s.as_ref()).collect()
    }

    pub fn sink_names(&self) -> Vec<&str> {
        self.sinks.keys().map(|s| s.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

/// Factory function: create a sink instance from config.
fn create_sink(config: &SinkInstanceConfig) -> Option<Box<dyn Sink>> {
    match config.sink_type.as_str() {
        "http_json" => Some(Box::new(http_json::HttpJsonSink::new(config))),
        "stdout" => Some(Box::new(stdout::StdoutSink::new(config))),
        _ => None,
    }
}
