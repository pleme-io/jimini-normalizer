use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::buffer::{BatchEmitter, EmitItem};
use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;
use crate::nats;
use crate::observability::{batch_size, dead_letters_total, normalizations_total};
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
    // Load config and providers once per request (lock-free)
    let config = state.config();
    let providers = state.providers();

    let provider = providers.get(&params.provider)?;

    let result = match NormalizationPipeline::run_with_pii(provider, &body, Some(&config.pii)) {
        Ok(r) => {
            normalizations_total()
                .with_label_values(&[&params.provider, "success"])
                .inc();
            r
        }
        Err(pipeline_err) => {
            normalizations_total()
                .with_label_values(&[&params.provider, "error"])
                .inc();

            // Publish error event to NATS error stream with retry
            let error_payload = build_error_payload(
                &params.provider,
                &body,
                &pipeline_err,
                &config.pii,
            );
            if let Err(e) = nats::publish_error(
                &state.js,
                &config.nats.error_stream_name,
                &params.provider,
                &error_payload,
                &config.retry,
            )
            .await
            {
                error!(error = %e, "failed to publish error to NATS after retries");
            }

            return Err(pipeline_err.app_error);
        }
    };

    // Build audit payload
    let audit_payload = result.audit.to_payload(
        provider.name(),
        provider.format(),
        result.assessments.iter().map(|a| a.scores.len()).sum::<usize>(),
    );

    // Publish each assessment to NATS with retry — data stays in memory
    // until NATS confirms receipt (JetStream ack).
    for assessment in &result.assessments {
        let msg = serde_json::json!({
            "assessment": assessment,
            "audit": audit_payload,
        });

        if let Err(e) = nats::publish_assessment(
            &state.js,
            &config.nats.stream_name,
            &params.provider,
            &msg,
            &config.retry,
        )
        .await
        {
            error!(error = %e, "failed to publish assessment to NATS after retries");
            return Err(AppError::Internal("failed to enqueue assessment".to_string()));
        }
    }

    info!(
        provider = params.provider,
        count = result.assessments.len(),
        "normalized and published assessments to NATS"
    );

    // Scrub patient_id for the API response if internal storage uses plaintext
    let mut response = result.assessments;
    if config.pii.should_store_patient_id_plaintext() {
        for a in &mut response {
            a.patient_id = crate::pii::scrub_patient_id(
                &a.patient_id,
                &config.pii.patient_id_strategy,
                &config.pii.hash_salt,
            );
        }
    }

    Ok(Json(response))
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

/// Batch normalize handler.
///
/// Memory lifecycle:
/// 1. Items are deserialized from JSON → owned Rust structs.
/// 2. Each item is normalized (pure CPU, no allocation beyond results).
/// 3. Results accumulate in a BatchEmitter buffer in memory.
/// 4. `emit_all()` drains the buffer and fan-outs to NATS in parallel
///    with bounded concurrency + exponential backoff retry.
/// 5. After N batches (configurable), the emitter shrinks its capacity.
/// 6. On function return, all local Vecs are dropped — Rust frees memory.
///
/// Data safety guarantee:
/// - Data lives in memory from ingestion until NATS JetStream ack.
/// - NATS JetStream provides at-least-once delivery to the worker.
/// - Worker persists to Postgres within an ACID transaction.
/// - Only after DB commit + NATS ack is the data considered stored.
pub async fn normalize_batch(
    State(state): State<Arc<AppState>>,
    Json(items): Json<Vec<BatchItem>>,
) -> (StatusCode, Json<BatchResponse>) {
    // Load config and providers once per request (lock-free)
    let config = state.config();
    let providers = state.providers();

    let mut successes = Vec::new();
    let mut errors = Vec::new();

    // Phase 1: Normalize all items — pure CPU work, accumulate results in memory
    let mut assessment_items: Vec<(String, serde_json::Value)> = Vec::new();
    let mut error_items: Vec<(String, serde_json::Value)> = Vec::new();

    batch_size()
        .with_label_values(&[])
        .observe(items.len() as f64);

    for (index, item) in items.into_iter().enumerate() {
        let raw = match serde_json::to_vec(&item.data) {
            Ok(raw) => raw,
            Err(e) => {
                errors.push(BatchError {
                    index,
                    provider: item.provider.clone(),
                    error: format!("failed to serialize data: {e}"),
                });
                error_items.push((
                    item.provider.clone(),
                    serde_json::json!({
                        "provider": item.provider,
                        "error_kind": "serialization",
                        "raw_input_hash": "",
                        "raw_input_ref": "[REDACTED]",
                        "error_message": format!("serialization: {e}"),
                        "violations": [],
                    }),
                ));
                continue;
            }
        };

        match providers.get(&item.provider) {
            Ok(provider) => match NormalizationPipeline::run_with_pii(provider, &raw, Some(&config.pii)) {
                Ok(result) => {
                    normalizations_total()
                        .with_label_values(&[&item.provider, "success"])
                        .inc();

                    let audit_payload = result.audit.to_payload(
                        provider.name(),
                        provider.format(),
                        result.assessments.iter().map(|a| a.scores.len()).sum::<usize>(),
                    );

                    for a in &result.assessments {
                        let msg = serde_json::json!({
                            "assessment": a,
                            "audit": audit_payload,
                        });
                        assessment_items.push((item.provider.clone(), msg));
                    }
                    successes.extend(result.assessments);
                }
                Err(pipeline_err) => {
                    normalizations_total()
                        .with_label_values(&[&item.provider, "error"])
                        .inc();

                    let error_payload = build_error_payload(
                        &item.provider,
                        &raw,
                        &pipeline_err,
                        &config.pii,
                    );
                    error_items.push((item.provider.clone(), error_payload));

                    errors.push(BatchError {
                        index,
                        provider: item.provider,
                        error: pipeline_err.app_error.to_string(),
                    });
                }
            },
            Err(e) => {
                let error_payload = serde_json::json!({
                    "provider": item.provider,
                    "error_kind": "unknown_provider",
                    "raw_input_hash": crate::audit::hash_input(&raw),
                    "raw_input_ref": crate::pii::process_raw_input(&raw, &config.pii),
                    "error_message": e.to_string(),
                    "violations": [],
                });
                error_items.push((item.provider.clone(), error_payload));

                errors.push(BatchError {
                    index,
                    provider: item.provider,
                    error: e.to_string(),
                });
            }
        }
    }

    // Phase 2: Parallel emit to NATS via BatchEmitter
    // Assessments and errors are emitted concurrently with bounded parallelism.
    let mut emitter = BatchEmitter::new(
        state.js.clone(),
        config.batch.clone(),
        config.retry.clone(),
    );

    // Buffer assessment messages
    for (provider, msg) in assessment_items {
        let subject = format!(
            "{}.{}",
            config.nats.stream_name.to_lowercase(),
            provider
        );
        match serde_json::to_vec(&msg) {
            Ok(data) => {
                emitter.push(EmitItem {
                    subject,
                    payload: data,
                });
            }
            Err(e) => {
                error!(error = %e, provider = %provider, "failed to serialize assessment for NATS, skipping");
            }
        }
    }

    // Buffer error messages
    for (provider, msg) in error_items {
        let subject = format!(
            "{}.{}",
            config.nats.error_stream_name.to_lowercase(),
            provider
        );
        match serde_json::to_vec(&msg) {
            Ok(data) => {
                emitter.push(EmitItem {
                    subject,
                    payload: data,
                });
            }
            Err(e) => {
                error!(error = %e, provider = %provider, "failed to serialize error for NATS, skipping");
            }
        }
    }

    // Emit all buffered items in parallel
    let report = emitter.emit_all().await;

    if report.failed > 0 {
        dead_letters_total().inc_by(report.dead_letters.len() as u64);
        warn!(
            succeeded = report.succeeded,
            failed = report.failed,
            "batch emit had failures — dead letters held in memory"
        );
        for dead in &report.dead_letters {
            error!(
                subject = dead.subject,
                payload_bytes = dead.payload.len(),
                "dead letter: failed to publish after all retries"
            );
        }
    }

    // Scrub patient_id in batch response if internal storage uses plaintext
    if config.pii.should_store_patient_id_plaintext() {
        for a in &mut successes {
            a.patient_id = crate::pii::scrub_patient_id(
                &a.patient_id,
                &config.pii.patient_id_strategy,
                &config.pii.hash_salt,
            );
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

/// Build error payload for NATS error stream.
///
/// **HIPAA/SOC2 compliant**: no raw input content is included — only a
/// SHA-256 hash for correlation. The `raw_input_preview` field that previously
/// exposed PHI has been removed.
fn build_error_payload(
    provider: &str,
    raw: &[u8],
    pipeline_err: &crate::pipeline::PipelineError,
    pii_config: &crate::pii::PiiConfig,
) -> serde_json::Value {
    let mut violations = Vec::new();
    for v in &pipeline_err.pre_violations {
        violations.push(serde_json::json!({
            "boundary": "pre_transform",
            "provider": provider,
            "field": v.field,
            "expected": v.expected,
            "actual": v.actual,
            "message": v.message,
        }));
    }
    for v in &pipeline_err.post_violations {
        violations.push(serde_json::json!({
            "boundary": "post_transform",
            "provider": provider,
            "field": v.field,
            "expected": v.expected,
            "actual": v.actual,
            "message": v.message,
        }));
    }

    serde_json::json!({
        "provider": provider,
        "error_kind": error_kind_from_app_error(&pipeline_err.app_error),
        "raw_input_hash": crate::audit::hash_input(raw),
        "raw_input_ref": crate::pii::process_raw_input(raw, pii_config),
        "error_message": pipeline_err.app_error.to_string(),
        "violations": violations,
    })
}

fn error_kind_from_app_error(err: &AppError) -> &'static str {
    match err {
        AppError::Validation(_) => "validation",
        AppError::UnknownProvider(_) => "unknown_provider",
        AppError::ParseError(_) => "parse",
        AppError::NotFound(_) => "not_found",
        AppError::Internal(_) => "internal",
        AppError::Database(_) => "database",
    }
}
