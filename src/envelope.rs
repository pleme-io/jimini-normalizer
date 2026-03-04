use serde::{Deserialize, Serialize};

use crate::model::unified::UnifiedAssessment;

/// Shared envelope for assessment events flowing through the outbox pipeline.
///
/// Used by:
/// - The API handler (serializes into NATS assessment stream)
/// - The worker (deserializes from NATS, forwards to outbox stream)
/// - The outbox worker (deserializes from outbox stream, persists via PostgresSink)
/// - The replay handler (constructs from re-processed failed records)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEnvelope {
    pub assessment: UnifiedAssessment,
    pub audit: AuditPayload,
}

/// Audit metadata captured during normalization pipeline execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditPayload {
    pub input_hash: String,
    pub input_size_bytes: i32,
    pub transformations: serde_json::Value,
    pub duration_us: i64,
    pub provider: String,
    pub source_format: String,
    pub score_count: i32,
}
