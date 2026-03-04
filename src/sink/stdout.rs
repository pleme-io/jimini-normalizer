use async_trait::async_trait;

use crate::config::SinkInstanceConfig;

use super::{Sink, SinkError};

/// Stdout sink for local debugging.
///
/// Prints JSON payloads to stdout. Useful for development and testing.
pub struct StdoutSink {
    name: String,
}

impl StdoutSink {
    pub fn new(config: &SinkInstanceConfig) -> Self {
        Self {
            name: config.name.clone(),
        }
    }
}

#[async_trait]
impl Sink for StdoutSink {
    fn name(&self) -> &str {
        &self.name
    }

    fn sink_type(&self) -> &str {
        "stdout"
    }

    async fn send(&self, payload: &serde_json::Value) -> Result<(), SinkError> {
        let json = serde_json::to_string_pretty(payload)
            .map_err(|e| SinkError::Serialization(e.to_string()))?;
        println!("[outbox:{}] {}", self.name, json);
        Ok(())
    }
}
