use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::failed_record;
use crate::envelope::{AuditPayload, OutboxEnvelope};
use crate::error::AppError;
use crate::failed;
use crate::observability::replay_total;
use crate::pipeline::NormalizationPipeline;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct FailedRecordsQuery {
    pub provider: Option<String>,
    pub error_kind: Option<String>,
    pub replayed: Option<bool>,
}

/// API-safe DTO for failed records. Never exposes raw_input directly —
/// only the hash for correlation. This is the HIPAA/SOC2 external boundary.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedRecordResponse {
    id: Uuid,
    provider: String,
    error_kind: String,
    error_detail: serde_json::Value,
    input_hash: String,
    replayed: bool,
    replayed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

fn entity_to_response(model: failed_record::Model) -> FailedRecordResponse {
    FailedRecordResponse {
        id: model.id,
        provider: model.provider,
        error_kind: model.error_kind,
        error_detail: model.error_detail,
        input_hash: model.input_hash,
        replayed: model.replayed,
        replayed_at: model.replayed_at,
        created_at: model.created_at,
    }
}

pub async fn list_failed_records(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FailedRecordsQuery>,
) -> Result<Json<Vec<FailedRecordResponse>>, AppError> {
    let mut query = failed_record::Entity::find();

    if let Some(ref provider) = params.provider {
        query = query.filter(failed_record::Column::Provider.eq(provider.as_str()));
    }
    if let Some(ref error_kind) = params.error_kind {
        query = query.filter(failed_record::Column::ErrorKind.eq(error_kind.as_str()));
    }
    if let Some(replayed) = params.replayed {
        query = query.filter(failed_record::Column::Replayed.eq(replayed));
    }

    let records = query
        .order_by_desc(failed_record::Column::CreatedAt)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(Json(records.into_iter().map(entity_to_response).collect()))
}

pub async fn get_failed_record(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<FailedRecordResponse>, AppError> {
    failed_record::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?
        .map(|m| Json(entity_to_response(m)))
        .ok_or_else(|| AppError::NotFound(format!("failed record {id} not found")))
}

/// Replay a failed record back through the pipeline.
///
/// **PII policy**:
/// - If `pii.internal.store_raw_for_replay: true` → raw input is available for replay.
/// - If `pii.internal.store_raw_for_replay: false` → only a hash/redacted marker is stored.
///   Replay requires the caller to re-submit via `/normalize`.
pub async fn replay_failed_record(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let config = state.config();
    let providers = state.providers();

    let record = failed_record::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("failed record {id} not found")))?;

    // Guard against re-replaying an already-replayed record
    if record.replayed {
        replay_total()
            .with_label_values(&["already_replayed"])
            .inc();
        return Ok(Json(serde_json::json!({
            "status": "already_replayed",
            "failed_record_id": id,
            "replayed_at": record.replayed_at,
        })));
    }

    if !config.pii.should_store_raw_input() {
        replay_total()
            .with_label_values(&["requires_resubmission"])
            .inc();
        return Ok(Json(serde_json::json!({
            "status": "replay_requires_resubmission",
            "failed_record_id": id,
            "provider": record.provider,
            "input_hash": record.input_hash,
            "error_kind": record.error_kind,
            "message": "Raw input is not stored (pii.internal.store_raw_for_replay is false). Re-submit the original payload to POST /normalize?provider=<provider>."
        })));
    }

    let provider = providers.get(&record.provider)?;
    let raw = record.raw_input.as_bytes();

    match NormalizationPipeline::run_with_pii(provider, raw, Some(&config.pii)) {
        Ok(result) => {
            // Persist each assessment via PostgresSink (ACID)
            for assessment in &result.assessments {
                // Build per-assessment audit with correct score_count
                let audit_json = result.audit.to_payload(
                    provider.name(),
                    provider.format(),
                    assessment.scores.len(),
                );
                let audit: AuditPayload = serde_json::from_value(audit_json)
                    .map_err(|e| AppError::Internal(format!("failed to build audit payload: {e}")))?;

                let envelope = OutboxEnvelope {
                    assessment: assessment.clone(),
                    audit,
                };

                state
                    .postgres_sink()
                    .persist(&envelope)
                    .await
                    .map_err(|e| AppError::Database(e.to_string()))?;

                // Best-effort: publish to outbox stream for external sink delivery.
                // If this fails, the sweeper will pick up the stale outbox entry.
                let config = state.config();
                match serde_json::to_value(&envelope) {
                    Ok(payload) => {
                        if let Err(e) = crate::nats::publish_outbox(
                            &state.js,
                            &config.outbox.stream_name,
                            &assessment.id,
                            &payload,
                            &config.retry,
                        )
                        .await
                        {
                            tracing::warn!(
                                error = %e,
                                assessment_id = %assessment.id,
                                "failed to publish replayed assessment to outbox — sweeper will reconcile"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            assessment_id = %assessment.id,
                            "failed to serialize replayed envelope for outbox"
                        );
                    }
                }
            }

            failed::mark_replayed(&state.db, id)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;

            replay_total()
                .with_label_values(&["replayed"])
                .inc();

            Ok(Json(serde_json::json!({
                "status": "replayed",
                "failed_record_id": id,
                "assessments_created": result.assessments.len()
            })))
        }
        Err(pipeline_err) => {
            replay_total()
                .with_label_values(&["error"])
                .inc();
            Err(pipeline_err.app_error)
        }
    }
}
