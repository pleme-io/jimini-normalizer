use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;
use crate::observability::{assessments_stored, normalizations_total};
use crate::pipeline::NormalizationPipeline;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct NormalizeParams {
    pub provider: String,
}

pub async fn normalize(
    State(state): State<Arc<AppState>>,
    Query(params): Query<NormalizeParams>,
    body: Bytes,
) -> Result<Json<Vec<UnifiedAssessment>>, AppError> {
    let provider = state.providers.get(&params.provider)?;

    let assessments = match NormalizationPipeline::run(provider, &body) {
        Ok(a) => {
            normalizations_total()
                .with_label_values(&[&params.provider, "success"])
                .inc();
            a
        }
        Err(e) => {
            normalizations_total()
                .with_label_values(&[&params.provider, "error"])
                .inc();
            return Err(e);
        }
    };

    // Store normalized assessments
    for assessment in &assessments {
        state
            .assessments
            .insert(assessment.id, assessment.clone());
        assessments_stored().inc();
    }

    info!(
        provider = params.provider,
        count = assessments.len(),
        "normalized and stored assessments"
    );

    Ok(Json(assessments))
}

#[derive(Deserialize)]
pub struct BatchItem {
    pub provider: String,
    pub data: serde_json::Value,
}

#[derive(Serialize)]
pub struct BatchResponse {
    pub successes: Vec<UnifiedAssessment>,
    pub errors: Vec<BatchError>,
}

#[derive(Serialize)]
pub struct BatchError {
    pub index: usize,
    pub provider: String,
    pub error: String,
}

pub async fn normalize_batch(
    State(state): State<Arc<AppState>>,
    Json(items): Json<Vec<BatchItem>>,
) -> (StatusCode, Json<BatchResponse>) {
    let mut successes = Vec::new();
    let mut errors = Vec::new();

    for (index, item) in items.into_iter().enumerate() {
        let raw = match serde_json::to_vec(&item.data) {
            Ok(raw) => raw,
            Err(e) => {
                errors.push(BatchError {
                    index,
                    provider: item.provider,
                    error: format!("failed to serialize data: {e}"),
                });
                continue;
            }
        };

        match state.providers.get(&item.provider) {
            Ok(provider) => match NormalizationPipeline::run(provider, &raw) {
                Ok(assessments) => {
                    normalizations_total()
                        .with_label_values(&[&item.provider, "success"])
                        .inc();
                    for a in &assessments {
                        state.assessments.insert(a.id, a.clone());
                        assessments_stored().inc();
                    }
                    successes.extend(assessments);
                }
                Err(e) => {
                    normalizations_total()
                        .with_label_values(&[&item.provider, "error"])
                        .inc();
                    errors.push(BatchError {
                        index,
                        provider: item.provider,
                        error: e.to_string(),
                    });
                }
            },
            Err(e) => {
                errors.push(BatchError {
                    index,
                    provider: item.provider,
                    error: e.to_string(),
                });
            }
        }
    }

    let status = if errors.is_empty() {
        StatusCode::OK
    } else if successes.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::MULTI_STATUS
    };

    (status, Json(BatchResponse { successes, errors }))
}
