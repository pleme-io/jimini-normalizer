use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::PullConsumer;
use chrono::Utc;
use futures::StreamExt;
use sea_orm::{DatabaseConnection, EntityTrait, Set, TransactionTrait};
use tracing::{error, info, warn};

use crate::config::{AppConfig, RetryConfig};
use crate::envelope::OutboxEnvelope;
use crate::observability::worker_messages_total;
use crate::state::AppState;

/// NATS message envelope for error events published by the API.
#[derive(serde::Deserialize)]
struct ErrorMessage {
    provider: String,
    error_kind: String,
    raw_input_hash: String,
    /// Raw input reference — either a SHA-256 hash, "[REDACTED]", or
    /// the original input when `pii.internal.store_raw_for_replay` is true.
    raw_input_ref: String,
    error_message: String,
    #[serde(default)]
    violations: Vec<ViolationPayload>,
}

#[derive(serde::Deserialize)]
struct ViolationPayload {
    boundary: String,
    #[serde(default)]
    #[allow(dead_code)]
    provider: String,
    field: String,
    expected: String,
    actual: String,
    message: String,
}

/// Run the worker: subscribe to assessment + error streams.
///
/// Assessment messages are forwarded to the outbox NATS stream for PostgresSink
/// persistence and external sink delivery. Error messages are persisted directly to DB.
pub async fn run(state: Arc<AppState>) {
    let config = state.config();

    let assessment_consumer = create_consumer(
        &state.js,
        &config.nats.stream_name,
        &format!("{}-assessments", config.nats.consumer_name),
        &config,
    )
    .await
    .expect("failed to create assessment consumer");

    let error_consumer = create_consumer(
        &state.js,
        &config.nats.error_stream_name,
        &format!("{}-errors", config.nats.consumer_name),
        &config,
    )
    .await
    .expect("failed to create error consumer");

    // Drop the config guard before spawning long-lived tasks
    drop(config);

    info!("worker started, consuming from assessment and error streams");

    let state_a = state.clone();
    let assessment_handle = tokio::spawn(async move {
        process_assessments(assessment_consumer, state_a).await;
    });

    let state_e = state.clone();
    let error_handle = tokio::spawn(async move {
        process_errors(error_consumer, state_e).await;
    });

    // Run both consumers concurrently; if either exits, the worker exits
    tokio::select! {
        _ = assessment_handle => {
            error!("assessment consumer exited unexpectedly");
        }
        _ = error_handle => {
            error!("error consumer exited unexpectedly");
        }
    }
}

async fn create_consumer(
    js: &jetstream::Context,
    stream_name: &str,
    consumer_name: &str,
    config: &AppConfig,
) -> Result<PullConsumer, async_nats::Error> {
    let stream = js.get_stream(stream_name).await?;

    let consumer = stream
        .get_or_create_consumer(
            consumer_name,
            jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.to_string()),
                ack_wait: Duration::from_secs(config.nats.ack_wait_secs),
                max_deliver: config.nats.max_deliver,
                max_ack_pending: config.nats.max_ack_pending,
                ..Default::default()
            },
        )
        .await?;

    info!(stream = stream_name, consumer = consumer_name, "created durable pull consumer");
    Ok(consumer)
}

/// Forward assessment messages to the outbox NATS stream.
///
/// The worker no longer persists to the database — that responsibility belongs
/// to the outbox worker via PostgresSink. The worker validates the envelope
/// and forwards the full payload (assessment + audit) to the outbox stream.
async fn process_assessments(
    consumer: PullConsumer,
    state: Arc<AppState>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to start assessment message stream");
            return;
        }
    };

    while let Some(msg_result) = messages.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "error receiving assessment message");
                continue;
            }
        };

        match serde_json::from_slice::<OutboxEnvelope>(&msg.payload) {
            Ok(envelope) => {
                let config = state.config();
                let assessment_id = envelope.assessment.id;

                // Forward the full envelope to the outbox stream.
                // The outbox worker will persist via PostgresSink and deliver to external sinks.
                match serde_json::to_value(&envelope) {
                    Ok(payload) => {
                        match crate::nats::publish_outbox(
                            &state.js,
                            &config.outbox.stream_name,
                            &assessment_id,
                            &payload,
                            &config.retry,
                        )
                        .await
                        {
                            Ok(()) => {
                                worker_messages_total()
                                    .with_label_values(&["assessment", "processed"])
                                    .inc();
                                if let Err(e) = msg.ack().await {
                                    error!(error = %e, "failed to ack assessment message");
                                }
                            }
                            Err(e) => {
                                worker_messages_total()
                                    .with_label_values(&["assessment", "error"])
                                    .inc();
                                error!(
                                    error = %e,
                                    %assessment_id,
                                    "failed to publish to outbox stream, nacking for redelivery"
                                );
                                if let Err(nak_err) =
                                    msg.ack_with(jetstream::AckKind::Nak(Some(Duration::from_secs(5)))).await
                                {
                                    error!(error = %nak_err, "failed to nak assessment message");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        worker_messages_total()
                            .with_label_values(&["assessment", "poison"])
                            .inc();
                        error!(
                            error = %e,
                            %assessment_id,
                            "failed to serialize envelope for outbox, terminating"
                        );
                        if let Err(term_err) = msg.ack_with(jetstream::AckKind::Term).await {
                            error!(error = %term_err, "failed to term assessment message");
                        }
                    }
                }
            }
            Err(e) => {
                worker_messages_total()
                    .with_label_values(&["assessment", "poison"])
                    .inc();
                error!(
                    error = %e,
                    subject = %msg.subject,
                    "failed to deserialize assessment message, terminating"
                );
                if let Err(term_err) = msg.ack_with(jetstream::AckKind::Term).await {
                    error!(error = %term_err, "failed to term poison assessment message");
                }
            }
        }
    }
}

// ─── Error Processing (unchanged — direct DB writes) ────────────────────────

async fn process_errors(
    consumer: PullConsumer,
    state: Arc<AppState>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to start error message stream");
            return;
        }
    };

    while let Some(msg_result) = messages.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "error receiving error message");
                continue;
            }
        };

        match serde_json::from_slice::<ErrorMessage>(&msg.payload) {
            Ok(envelope) => {
                let retry_cfg = state.config().retry.clone();
                match persist_error_with_retry(&state.db, &envelope, &retry_cfg).await {
                    Ok(()) => {
                        worker_messages_total()
                            .with_label_values(&["error", "processed"])
                            .inc();
                        if let Err(e) = msg.ack().await {
                            error!(error = %e, "failed to ack error message");
                        }
                    }
                    Err(e) => {
                        worker_messages_total()
                            .with_label_values(&["error", "error"])
                            .inc();
                        error!(error = %e, "failed to persist error after retries, nacking for redelivery");
                        if let Err(nak_err) = msg.ack_with(jetstream::AckKind::Nak(Some(Duration::from_secs(5)))).await {
                            error!(error = %nak_err, "failed to nak error message");
                        }
                    }
                }
            }
            Err(e) => {
                worker_messages_total()
                    .with_label_values(&["error", "poison"])
                    .inc();
                error!(
                    error = %e,
                    subject = %msg.subject,
                    "failed to deserialize error message, terminating"
                );
                if let Err(term_err) = msg.ack_with(jetstream::AckKind::Term).await {
                    error!(error = %term_err, "failed to term poison error message");
                }
            }
        }
    }
}

async fn persist_error_with_retry(
    db: &DatabaseConnection,
    envelope: &ErrorMessage,
    retry_cfg: &RetryConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut attempt = 0u32;

    loop {
        attempt += 1;
        match persist_error(db, envelope).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt < retry_cfg.max_attempts && is_transient_db_error(&e) => {
                let delay = backoff_delay(attempt, retry_cfg);
                warn!(
                    attempt,
                    max = retry_cfg.max_attempts,
                    delay_ms = delay.as_millis() as u64,
                    error = %e,
                    "transient DB error persisting error record, retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn persist_error(
    db: &DatabaseConnection,
    envelope: &ErrorMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::entity::{failed_record, schema_violation};

    let txn = db.begin().await?;

    // Insert failed record — idempotent: ON CONFLICT (input_hash, provider) DO NOTHING
    let error_detail = serde_json::json!({
        "error_kind": envelope.error_kind,
        "error_message": envelope.error_message,
    });
    let failed = failed_record::ActiveModel {
        id: Set(uuid::Uuid::new_v4()),
        provider: Set(envelope.provider.clone()),
        raw_input: Set(envelope.raw_input_ref.clone()),
        error_kind: Set(envelope.error_kind.clone()),
        error_detail: Set(error_detail),
        input_hash: Set(envelope.raw_input_hash.clone()),
        replayed: Set(false),
        replayed_at: Set(None),
        created_at: Set(Utc::now().into()),
    };
    failed_record::Entity::insert(failed)
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([
                failed_record::Column::InputHash,
                failed_record::Column::Provider,
            ])
            .do_nothing()
            .to_owned(),
        )
        .do_nothing()
        .exec(&txn)
        .await?;

    // Insert schema violations if any (grouped by boundary)
    if !envelope.violations.is_empty() {
        let mut by_boundary: std::collections::HashMap<String, Vec<serde_json::Value>> =
            std::collections::HashMap::new();
        for v in &envelope.violations {
            by_boundary
                .entry(v.boundary.clone())
                .or_default()
                .push(serde_json::json!({
                    "field": v.field,
                    "expected": v.expected,
                    "actual": v.actual,
                    "message": v.message,
                }));
        }

        for (boundary, violations_json) in by_boundary {
            let violation = schema_violation::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                boundary: Set(boundary),
                provider: Set(envelope.provider.clone()),
                input_hash: Set(envelope.raw_input_hash.clone()),
                violations: Set(serde_json::Value::Array(violations_json)),
                raw_input: Set(envelope.raw_input_ref.clone()),
                created_at: Set(Utc::now().into()),
            };
            schema_violation::Entity::insert(violation)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        schema_violation::Column::Boundary,
                        schema_violation::Column::Provider,
                        schema_violation::Column::InputHash,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .do_nothing()
                .exec(&txn)
                .await?;
        }
    }

    txn.commit().await?;
    Ok(())
}

/// Detect transient DB errors that are safe to retry:
/// connection refused, timeout, serialization failure (Postgres 40001).
fn is_transient_db_error(err: &Box<dyn std::error::Error + Send + Sync>) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("broken pipe")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("could not serialize access")
        || msg.contains("too many connections")
        || msg.contains("pool timed out")
}

fn backoff_delay(attempt: u32, cfg: &RetryConfig) -> Duration {
    let base = cfg.base_delay_ms as u128;
    let exp = base.saturating_mul(2u128.saturating_pow(attempt.saturating_sub(1)));
    let capped = exp.min(cfg.max_delay_ms as u128) as u64;
    let jitter = capped / 4;
    let actual = capped.saturating_sub(jitter / 2)
        + (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64
            % jitter.max(1));
    Duration::from_millis(actual)
}
