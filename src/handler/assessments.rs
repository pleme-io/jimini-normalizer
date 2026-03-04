use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::entity::assessment;
use crate::error::AppError;
use crate::model::unified::{AssessmentMetadata, Score, UnifiedAssessment};
use crate::pii;
use crate::state::AppState;

pub async fn list_assessments(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<UnifiedAssessment>>, AppError> {
    let config = state.config();

    let records = assessment::Entity::find()
        .all(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    let assessments: Vec<UnifiedAssessment> = records
        .into_iter()
        .map(|m| entity_to_unified(m, &config.pii))
        .collect();

    Ok(Json(assessments))
}

pub async fn get_assessment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<UnifiedAssessment>, AppError> {
    let config = state.config();

    let record = assessment::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("assessment {id} not found")))?;

    Ok(Json(entity_to_unified(record, &config.pii)))
}

/// Convert DB entity to API response.
///
/// **HIPAA/SOC2 note**: API responses ALWAYS scrub patient_id, even when it's
/// stored plaintext internally (for internal replay/lookup). The scrubbing
/// strategy is applied here at the external boundary.
fn entity_to_unified(model: assessment::Model, pii_config: &pii::PiiConfig) -> UnifiedAssessment {
    let scores: Vec<Score> =
        serde_json::from_value(model.scores).unwrap_or_default();

    // Always scrub patient_id in API responses — even if stored plaintext internally
    let patient_id = if pii_config.should_store_patient_id_plaintext() {
        pii::scrub_patient_id(&model.patient_id, &pii_config.patient_id_strategy, &pii_config.hash_salt)
    } else {
        // Already scrubbed at pipeline time
        model.patient_id
    };

    UnifiedAssessment {
        id: model.id,
        patient_id,
        assessment_date: model.assessment_date,
        assessment_type: model.assessment_type,
        scores,
        metadata: AssessmentMetadata {
            source_provider: model.source_provider,
            source_format: model.source_format,
            ingested_at: model.ingested_at,
            version: model.version,
        },
    }
}
