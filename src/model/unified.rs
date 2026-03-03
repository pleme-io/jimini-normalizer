use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, FieldError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedAssessment {
    pub id: Uuid,
    pub patient_id: String,
    pub assessment_date: DateTime<Utc>,
    pub assessment_type: String,
    pub scores: Vec<Score>,
    pub metadata: AssessmentMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    pub dimension: String,
    pub value: f64,
    pub scale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentMetadata {
    pub source_provider: String,
    pub source_format: String,
    pub ingested_at: DateTime<Utc>,
    pub version: String,
}

impl UnifiedAssessment {
    pub fn validate(&self) -> Result<(), AppError> {
        let mut errors = Vec::new();

        if self.patient_id.is_empty() {
            errors.push(FieldError {
                field: "patient_id".into(),
                message: "patient_id is required".into(),
            });
        }

        if self.assessment_type.is_empty() {
            errors.push(FieldError {
                field: "assessment_type".into(),
                message: "assessment_type is required".into(),
            });
        }

        if self.scores.is_empty() {
            errors.push(FieldError {
                field: "scores".into(),
                message: "at least one score is required".into(),
            });
        }

        for (i, score) in self.scores.iter().enumerate() {
            if !(0.0..=100.0).contains(&score.value) {
                errors.push(FieldError {
                    field: format!("scores[{i}].value"),
                    message: format!(
                        "score must be between 0 and 100, got {}",
                        score.value
                    ),
                });
            }
            if score.dimension.is_empty() {
                errors.push(FieldError {
                    field: format!("scores[{i}].dimension"),
                    message: "dimension is required".into(),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Validation(errors))
        }
    }
}
