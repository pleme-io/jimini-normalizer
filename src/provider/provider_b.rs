use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::AppError;
use crate::model::provider_b::ProviderBPayload;
use crate::model::unified::{AssessmentMetadata, Score, UnifiedAssessment};
use crate::provider::NormalizationProvider;

/// Provider B: Flat key-value format with scores already on 0-100 scale.
pub struct ProviderB;

impl NormalizationProvider for ProviderB {
    fn name(&self) -> &str {
        "provider_b"
    }

    fn format(&self) -> &str {
        "json_flat"
    }

    fn validate_input(&self, raw: &[u8]) -> Result<(), AppError> {
        serde_json::from_slice::<ProviderBPayload>(raw)
            .map(|_| ())
            .map_err(|e| AppError::ParseError(e.to_string()))
    }

    fn normalize(&self, raw: &[u8]) -> Result<Vec<UnifiedAssessment>, AppError> {
        let payload: ProviderBPayload =
            serde_json::from_slice(raw).map_err(|e| AppError::ParseError(e.to_string()))?;

        let assessment_date: DateTime<Utc> = payload
            .assessment_date
            .parse()
            .map_err(|e: chrono::ParseError| AppError::ParseError(e.to_string()))?;

        let now = Utc::now();
        let scores = payload
            .scores
            .into_iter()
            .map(|(dimension, value)| Score {
                dimension,
                value, // already 0-100
                scale: "0-100".to_string(),
            })
            .collect();

        let assessment = UnifiedAssessment {
            id: Uuid::new_v4(),
            patient_id: payload.patient_id,
            assessment_date,
            assessment_type: payload.assessment_type,
            scores,
            metadata: AssessmentMetadata {
                source_provider: self.name().to_string(),
                source_format: self.format().to_string(),
                ingested_at: now,
                version: "1.0".to_string(),
            },
        };

        Ok(vec![assessment])
    }
}
