use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use uuid::Uuid;

use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;
use crate::state::AppState;

pub async fn list_assessments(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<UnifiedAssessment>> {
    let assessments: Vec<UnifiedAssessment> = state
        .assessments
        .iter()
        .map(|entry| entry.value().clone())
        .collect();
    Json(assessments)
}

pub async fn get_assessment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<UnifiedAssessment>, AppError> {
    state
        .assessments
        .get(&id)
        .map(|entry| Json(entry.value().clone()))
        .ok_or_else(|| AppError::NotFound(format!("assessment {id} not found")))
}
