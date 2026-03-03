use chrono::Utc;
use uuid::Uuid;

use crate::error::AppError;
use crate::model::provider_a::ProviderAPayload;
use crate::model::unified::{AssessmentMetadata, Score, UnifiedAssessment};
use crate::provider::NormalizationProvider;

/// Provider A: Nested JSON format with scores on a 0-10 scale.
pub struct ProviderA;

impl NormalizationProvider for ProviderA {
    fn name(&self) -> &str {
        "provider_a"
    }

    fn format(&self) -> &str {
        "json_nested"
    }

    fn validate_input(&self, raw: &[u8]) -> Result<(), AppError> {
        serde_json::from_slice::<ProviderAPayload>(raw)
            .map(|_| ())
            .map_err(|e| AppError::ParseError(e.to_string()))
    }

    fn normalize(&self, raw: &[u8]) -> Result<Vec<UnifiedAssessment>, AppError> {
        let payload: ProviderAPayload =
            serde_json::from_slice(raw).map_err(|e| AppError::ParseError(e.to_string()))?;

        let now = Utc::now();
        let scores = payload
            .assessment
            .scores
            .into_iter()
            .map(|s| Score {
                dimension: s.category,
                value: s.value * 10.0, // 0-10 → 0-100
                scale: "0-100".to_string(),
            })
            .collect();

        let assessment = UnifiedAssessment {
            id: Uuid::new_v4(),
            patient_id: payload.patient.id,
            assessment_date: payload.assessment.date,
            assessment_type: payload.assessment.assessment_type,
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
