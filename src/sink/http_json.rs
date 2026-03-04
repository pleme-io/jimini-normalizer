use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;

use crate::config::SinkInstanceConfig;

use super::{Sink, SinkError};

/// HTTP JSON sink for Vector/OTEL endpoints.
///
/// Sends JSON payloads via HTTP POST with configurable headers and timeout.
pub struct HttpJsonSink {
    name: String,
    endpoint: String,
    timeout: Duration,
    headers: HashMap<String, String>,
    client: reqwest::Client,
}

impl HttpJsonSink {
    pub fn new(config: &SinkInstanceConfig) -> Self {
        Self {
            name: config.name.clone(),
            endpoint: config.endpoint.clone(),
            timeout: Duration::from_secs(config.request_timeout_secs),
            headers: config.headers.clone(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Sink for HttpJsonSink {
    fn name(&self) -> &str {
        &self.name
    }

    fn sink_type(&self) -> &str {
        "http_json"
    }

    async fn send(&self, payload: &serde_json::Value) -> Result<(), SinkError> {
        if self.endpoint.is_empty() {
            return Err(SinkError::NotConfigured(format!(
                "sink '{}' has no endpoint configured",
                self.name
            )));
        }

        let mut req = self
            .client
            .post(&self.endpoint)
            .json(payload)
            .timeout(self.timeout);

        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        let resp = req.send().await.map_err(|e| SinkError::Http(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(SinkError::Http(format!(
                "sink '{}' returned status {}",
                self.name,
                resp.status()
            )))
        }
    }
}
