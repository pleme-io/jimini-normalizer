use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::{normalization_audit_log, schema_violation};
use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct AuditQuery {
    pub provider: Option<String>,
    pub assessment_id: Option<Uuid>,
}

pub async fn list_audit_logs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditQuery>,
) -> Result<Json<Vec<normalization_audit_log::Model>>, AppError> {
    let mut query = normalization_audit_log::Entity::find();

    if let Some(ref provider) = params.provider {
        query = query.filter(normalization_audit_log::Column::Provider.eq(provider.as_str()));
    }
    if let Some(assessment_id) = params.assessment_id {
        query =
            query.filter(normalization_audit_log::Column::AssessmentId.eq(assessment_id));
    }

    let records = query
        .order_by_desc(normalization_audit_log::Column::CreatedAt)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(Json(records))
}

#[derive(Deserialize)]
pub struct ViolationsQuery {
    pub boundary: Option<String>,
    pub provider: Option<String>,
}

/// API-safe DTO for schema violations. Never exposes raw_input directly —
/// only the hash for correlation. This is the HIPAA/SOC2 external boundary.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaViolationResponse {
    id: Uuid,
    boundary: String,
    provider: String,
    input_hash: String,
    violations: serde_json::Value,
    created_at: DateTime<Utc>,
}

fn violation_to_response(model: schema_violation::Model) -> SchemaViolationResponse {
    SchemaViolationResponse {
        id: model.id,
        boundary: model.boundary,
        provider: model.provider,
        input_hash: model.input_hash,
        violations: model.violations,
        created_at: model.created_at,
    }
}

pub async fn list_schema_violations(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ViolationsQuery>,
) -> Result<Json<Vec<SchemaViolationResponse>>, AppError> {
    let mut query = schema_violation::Entity::find();

    if let Some(ref boundary) = params.boundary {
        query = query.filter(schema_violation::Column::Boundary.eq(boundary.as_str()));
    }
    if let Some(ref provider) = params.provider {
        query = query.filter(schema_violation::Column::Provider.eq(provider.as_str()));
    }

    let records = query
        .order_by_desc(schema_violation::Column::CreatedAt)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(Json(records.into_iter().map(violation_to_response).collect()))
}
