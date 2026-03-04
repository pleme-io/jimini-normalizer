use std::time::Instant;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, Set};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::entity::normalization_audit_log;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub step: String,
    pub duration_us: u64,
    #[serde(flatten)]
    pub details: serde_json::Value,
}

/// Collects timing and metadata during pipeline execution for audit persistence.
pub struct AuditBuilder {
    start: Instant,
    steps: Vec<StepRecord>,
    input_hash: String,
    input_size: usize,
}

impl AuditBuilder {
    pub fn new(raw_input: &[u8]) -> Self {
        let hash = Sha256::digest(raw_input);
        Self {
            start: Instant::now(),
            steps: Vec::new(),
            input_hash: format!("{hash:x}"),
            input_size: raw_input.len(),
        }
    }

    pub fn record_step(&mut self, step: &str, started: Instant, details: serde_json::Value) {
        self.steps.push(StepRecord {
            step: step.to_string(),
            duration_us: started.elapsed().as_micros() as u64,
            details,
        });
    }

    pub fn input_hash(&self) -> &str {
        &self.input_hash
    }

    pub fn input_size(&self) -> usize {
        self.input_size
    }

    /// Build a serializable payload for NATS message envelope.
    pub fn to_payload(&self, provider: &str, source_format: &str, score_count: usize) -> serde_json::Value {
        serde_json::json!({
            "input_hash": self.input_hash,
            "input_size_bytes": self.input_size as i32,
            "transformations": serde_json::to_value(&self.steps).unwrap_or_default(),
            "duration_us": self.start.elapsed().as_micros() as i64,
            "provider": provider,
            "source_format": source_format,
            "score_count": score_count as i32,
        })
    }

    pub async fn persist(
        &self,
        db: &impl ConnectionTrait,
        assessment_id: Uuid,
        provider: &str,
        source_format: &str,
        score_count: usize,
    ) -> Result<(), sea_orm::DbErr> {
        let model = normalization_audit_log::ActiveModel {
            id: Set(Uuid::new_v4()),
            assessment_id: Set(assessment_id),
            provider: Set(provider.to_string()),
            source_format: Set(source_format.to_string()),
            input_hash: Set(self.input_hash.clone()),
            input_size_bytes: Set(self.input_size as i32),
            score_count: Set(score_count as i32),
            transformations: Set(serde_json::to_value(&self.steps).unwrap_or_default()),
            duration_us: Set(self.start.elapsed().as_micros() as i64),
            created_at: Set(Utc::now().into()),
        };
        model.insert(db).await?;
        Ok(())
    }
}

pub fn hash_input(raw: &[u8]) -> String {
    let hash = Sha256::digest(raw);
    format!("{hash:x}")
}
