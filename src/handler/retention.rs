use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use crate::error::AppError;
use crate::retention;
use crate::state::AppState;

/// Trigger retention purge on-demand (also called by K8s CronJob via curl).
pub async fn purge(
    State(state): State<Arc<AppState>>,
) -> Result<Json<retention::PurgeReport>, AppError> {
    let config = state.config();

    let report = retention::purge_expired(&state.db, config.retention.days)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(Json(report))
}
