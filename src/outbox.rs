use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream;
use async_nats::jetstream::consumer::PullConsumer;
use chrono::Utc;
use futures::StreamExt;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set};
use tracing::{debug, error, info, warn};

use crate::config::RetryConfig;
use crate::entity::outbox as outbox_entity;
use crate::envelope::OutboxEnvelope;
use crate::observability::{
    outbox_processed_total, outbox_restarts_total, sink_delivery_duration, sink_delivery_total,
    sweeper_entries_failed, sweeper_entries_redriven, sweeper_runs_total,
};
use crate::state::AppState;

/// Insert an outbox record within an existing transaction.
///
/// Idempotent: ON CONFLICT (assessment_id) DO NOTHING — safe on NATS redelivery.
/// The durable DB record ensures the reconciliation sweeper can re-drive delivery
/// if the NATS outbox publish fails after the transaction commits.
pub async fn enqueue_in_txn<C: sea_orm::ConnectionTrait>(
    txn: &C,
    assessment_id: uuid::Uuid,
    payload: serde_json::Value,
) -> Result<(), sea_orm::DbErr> {
    let model = outbox_entity::ActiveModel {
        id: Set(uuid::Uuid::new_v4()),
        assessment_id: Set(assessment_id),
        payload: Set(payload),
        status: Set("pending".to_string()),
        retries: Set(0),
        created_at: Set(Utc::now().into()),
        flushed_at: Set(None),
    };
    outbox_entity::Entity::insert(model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(outbox_entity::Column::AssessmentId)
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec(txn)
        .await?;
    Ok(())
}

// ─── Outbox Worker (NATS consumer → PostgresSink → external sink delivery) ──

/// Run the outbox worker with automatic restart on failure.
///
/// Wraps `run_outbox_worker_inner` in a retry loop with exponential backoff.
/// Transient failures (NATS disconnect, stream not found) are retried indefinitely.
/// Backoff resets after 60 seconds of successful operation.
pub async fn run_outbox_worker(state: Arc<AppState>) {
    let mut attempt = 0u32;
    let max_backoff = Duration::from_secs(60);

    loop {
        info!("starting outbox worker");
        let start = tokio::time::Instant::now();
        run_outbox_worker_inner(state.clone()).await;

        // Reset backoff if the worker ran for at least 60 seconds (was healthy)
        if start.elapsed() > Duration::from_secs(60) {
            attempt = 0;
        }

        // Worker exited — restart with backoff
        outbox_restarts_total().inc();
        attempt += 1;
        let delay = Duration::from_secs(
            (2u64.saturating_pow(attempt.min(6))).min(max_backoff.as_secs()),
        );
        warn!(
            attempt,
            delay_secs = delay.as_secs(),
            "outbox worker exited, restarting after backoff"
        );
        tokio::time::sleep(delay).await;
    }
}

async fn run_outbox_worker_inner(state: Arc<AppState>) {
    let config = state.config();
    let stream_name = config.outbox.stream_name.clone();
    let consumer_name = config.outbox.consumer_name.clone();
    let ack_wait_secs = config.nats.ack_wait_secs;
    let max_deliver = config.nats.max_deliver;
    let max_ack_pending = config.nats.max_ack_pending;
    drop(config);

    let stream = match state.js.get_stream(&stream_name).await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, stream = %stream_name, "failed to get outbox stream");
            return;
        }
    };

    let consumer: PullConsumer = match stream
        .get_or_create_consumer(
            &consumer_name,
            jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                ack_wait: Duration::from_secs(ack_wait_secs),
                max_deliver,
                max_ack_pending,
                ..Default::default()
            },
        )
        .await
    {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "failed to create outbox consumer");
            return;
        }
    };

    info!(consumer = %consumer_name, "outbox consumer ready");

    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to start outbox message stream");
            return;
        }
    };

    while let Some(msg_result) = messages.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "error receiving outbox message");
                continue;
            }
        };

        // Phase 1: Deserialize and persist via PostgresSink
        match serde_json::from_slice::<OutboxEnvelope>(&msg.payload) {
            Ok(envelope) => {
                let assessment_id = envelope.assessment.id;

                // PostgresSink is the primary, mandatory sink — if it fails, NAK for redelivery
                if let Err(e) = state.postgres_sink().persist(&envelope).await {
                    outbox_processed_total()
                        .with_label_values(&["postgres_failed"])
                        .inc();
                    error!(
                        error = %e,
                        %assessment_id,
                        "PostgresSink failed, nacking for redelivery"
                    );
                    if let Err(nak_err) = msg.ack_with(jetstream::AckKind::Nak(Some(Duration::from_secs(5)))).await {
                        error!(error = %nak_err, "failed to nak outbox message");
                    }
                    continue;
                }

                // Phase 2: Deliver to external sinks (assessment-only JSON)
                let retry_cfg = state.config().retry.clone();
                let assessment_payload = match serde_json::to_value(&envelope.assessment) {
                    Ok(p) => p,
                    Err(e) => {
                        error!(error = %e, "failed to serialize assessment for external sinks");
                        // Data safe in DB — ACK the message
                        if let Err(e) = msg.ack().await {
                            error!(error = %e, "failed to ack outbox message");
                        }
                        continue;
                    }
                };

                let delivery = deliver_to_sinks(&state, &assessment_payload, &retry_cfg).await;

                match delivery {
                    DeliveryResult::AllSucceeded => {
                        outbox_processed_total()
                            .with_label_values(&["persisted"])
                            .inc();
                        if let Err(e) = mark_flushed(&state.db, assessment_id).await {
                            warn!(error = %e, "failed to mark outbox entry as flushed");
                        }
                        if let Err(e) = msg.ack().await {
                            error!(error = %e, "failed to ack outbox message");
                        }
                    }
                    DeliveryResult::NoSinksConfigured => {
                        outbox_processed_total()
                            .with_label_values(&["persisted"])
                            .inc();
                        if let Err(e) = mark_flushed(&state.db, assessment_id).await {
                            warn!(error = %e, "failed to mark outbox entry as flushed");
                        }
                        if let Err(e) = msg.ack().await {
                            error!(error = %e, "failed to ack outbox message (no sinks)");
                        }
                    }
                    DeliveryResult::AllFailed(e) => {
                        outbox_processed_total()
                            .with_label_values(&["sink_failed"])
                            .inc();
                        warn!(
                            error = %e,
                            "all external sinks failed — data safe in DB, sweeper will retry"
                        );
                        if let Err(e) = msg.ack().await {
                            error!(error = %e, "failed to ack outbox message");
                        }
                    }
                    DeliveryResult::PartialFailure { succeeded, failed } => {
                        outbox_processed_total()
                            .with_label_values(&["sink_failed"])
                            .inc();
                        warn!(
                            succeeded = succeeded,
                            failed = failed,
                            "partial sink failure — data safe in DB, sweeper will retry"
                        );
                        if let Err(e) = msg.ack().await {
                            error!(error = %e, "failed to ack outbox message after partial delivery");
                        }
                    }
                }
            }
            Err(e) => {
                // Try legacy format: assessment-only payload (backward compat)
                match serde_json::from_slice::<serde_json::Value>(&msg.payload) {
                    Ok(payload) => {
                        // Legacy format — no audit data, just deliver to external sinks.
                        // No PostgresSink persist (no audit data to build an envelope).
                        let retry_cfg = state.config().retry.clone();
                        let delivery = deliver_to_sinks(&state, &payload, &retry_cfg).await;

                        let assessment_id = payload
                            .get("id")
                            .and_then(|v| v.as_str())
                            .and_then(|s| uuid::Uuid::parse_str(s).ok());

                        match delivery {
                            DeliveryResult::AllSucceeded => {
                                if let Some(id) = assessment_id {
                                    if let Err(e) = mark_flushed(&state.db, id).await {
                                        warn!(error = %e, "failed to mark outbox entry as flushed");
                                    }
                                }
                                if let Err(e) = msg.ack().await {
                                    error!(error = %e, "failed to ack outbox message");
                                }
                            }
                            DeliveryResult::NoSinksConfigured => {
                                if let Some(id) = assessment_id {
                                    if let Err(e) = mark_flushed(&state.db, id).await {
                                        warn!(error = %e, "failed to mark outbox entry as flushed");
                                    }
                                }
                                if let Err(e) = msg.ack().await {
                                    error!(error = %e, "failed to ack outbox message (no sinks)");
                                }
                            }
                            DeliveryResult::AllFailed(_) | DeliveryResult::PartialFailure { .. } => {
                                // Legacy messages have no DB outbox entry — NAK for
                                // NATS redelivery so the message isn't permanently lost.
                                warn!("legacy outbox message sink delivery failed, nacking for redelivery");
                                if let Err(e) = msg.ack_with(jetstream::AckKind::Nak(Some(Duration::from_secs(10)))).await {
                                    error!(error = %e, "failed to nak legacy outbox message");
                                }
                            }
                        }
                    }
                    Err(_) => {
                        outbox_processed_total()
                            .with_label_values(&["poison"])
                            .inc();
                        error!(
                            error = %e,
                            subject = %msg.subject,
                            "failed to deserialize outbox message, terminating"
                        );
                        if let Err(term_err) = msg.ack_with(jetstream::AckKind::Term).await {
                            error!(error = %term_err, "failed to term outbox message");
                        }
                    }
                }
            }
        }
    }
}

enum DeliveryResult {
    AllSucceeded,
    NoSinksConfigured,
    AllFailed(String),
    PartialFailure { succeeded: usize, failed: usize },
}

/// Deliver a payload to all enabled external sinks with per-sink retry.
async fn deliver_to_sinks(
    state: &AppState,
    payload: &serde_json::Value,
    retry_cfg: &RetryConfig,
) -> DeliveryResult {
    let sinks = state.sinks();
    let all_sinks = sinks.all();

    if all_sinks.is_empty() {
        debug!("no external sinks configured");
        return DeliveryResult::NoSinksConfigured;
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut last_error = String::new();

    for sink in &all_sinks {
        let mut attempt = 0u32;
        let sink_start = Instant::now();

        loop {
            attempt += 1;
            match sink.send(payload).await {
                Ok(()) => {
                    sink_delivery_duration()
                        .with_label_values(&[sink.name()])
                        .observe(sink_start.elapsed().as_secs_f64());
                    sink_delivery_total()
                        .with_label_values(&[sink.name(), "success"])
                        .inc();
                    debug!(sink = sink.name(), "delivered to sink");
                    succeeded += 1;
                    break;
                }
                Err(e) if attempt < retry_cfg.max_attempts => {
                    let delay = backoff_delay(attempt, retry_cfg);
                    warn!(
                        sink = sink.name(),
                        attempt,
                        max = retry_cfg.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "sink delivery failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    sink_delivery_duration()
                        .with_label_values(&[sink.name()])
                        .observe(sink_start.elapsed().as_secs_f64());
                    sink_delivery_total()
                        .with_label_values(&[sink.name(), "error"])
                        .inc();
                    error!(
                        sink = sink.name(),
                        attempt,
                        error = %e,
                        "sink delivery failed after all retries"
                    );
                    last_error = format!("sink '{}': {}", sink.name(), e);
                    failed += 1;
                    break;
                }
            }
        }
    }

    if failed == 0 {
        DeliveryResult::AllSucceeded
    } else if succeeded == 0 {
        DeliveryResult::AllFailed(last_error)
    } else {
        DeliveryResult::PartialFailure { succeeded, failed }
    }
}

// ─── Reconciliation Sweeper ─────────────────────────────────────────────────

/// Periodically scan for stale "pending" outbox entries and deliver them
/// directly to external sinks.
///
/// This closes the gap where:
/// - The outbox worker acked after PostgresSink succeeded but external sinks failed
/// - The outbox worker crashed between PostgresSink and external sink delivery
///
/// The sweeper calls deliver_to_sinks() directly — no NATS re-publish needed
/// since PostgresSink has already persisted the data.
pub async fn run_reconciliation_sweeper(state: Arc<AppState>) {
    // Initial delay to let streams and consumers initialize
    tokio::time::sleep(Duration::from_secs(10)).await;

    loop {
        let config = state.config();
        let interval = config.outbox.sweeper_interval_secs;
        let stale_secs = config.outbox.sweeper_stale_after_secs;
        drop(config);

        tokio::time::sleep(Duration::from_secs(interval)).await;

        match sweep_stale_entries(&state, stale_secs).await {
            Ok(count) => {
                sweeper_runs_total()
                    .with_label_values(&["ok"])
                    .inc();
                if count > 0 {
                    info!(count, "reconciliation sweeper re-delivered stale outbox entries");
                }
            }
            Err(e) => {
                sweeper_runs_total()
                    .with_label_values(&["error"])
                    .inc();
                warn!(error = %e, "reconciliation sweeper failed");
            }
        }
    }
}

async fn sweep_stale_entries(
    state: &AppState,
    stale_secs: u64,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let cutoff = Utc::now() - chrono::Duration::seconds(stale_secs as i64);

    let stale = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::Status.eq("pending"))
        .filter(outbox_entity::Column::CreatedAt.lt(cutoff))
        .order_by_asc(outbox_entity::Column::CreatedAt)
        .limit(100) // Bound memory usage on large backlogs
        .all(&state.db)
        .await?;

    if stale.is_empty() {
        return Ok(0);
    }

    let count = stale.len();
    debug!(count, "found stale outbox entries for reconciliation");

    let retry_cfg = state.config().retry.clone();

    for entry in stale {
        // The stored payload may be either:
        // 1. OutboxEnvelope (new format: assessment + audit) — extract assessment for sinks
        // 2. Assessment-only JSON (legacy format) — use directly
        let sink_payload = if let Ok(envelope) =
            serde_json::from_value::<OutboxEnvelope>(entry.payload.clone())
        {
            serde_json::to_value(&envelope.assessment).unwrap_or(entry.payload.clone())
        } else {
            entry.payload.clone()
        };

        let delivery = deliver_to_sinks(state, &sink_payload, &retry_cfg).await;

        match delivery {
            DeliveryResult::AllSucceeded | DeliveryResult::NoSinksConfigured => {
                sweeper_entries_redriven().inc();
                if let Err(e) = mark_flushed(&state.db, entry.assessment_id).await {
                    warn!(
                        error = %e,
                        assessment_id = %entry.assessment_id,
                        "sweeper: failed to mark outbox entry as flushed"
                    );
                }
            }
            DeliveryResult::AllFailed(ref e) => {
                warn!(
                    error = %e,
                    assessment_id = %entry.assessment_id,
                    "sweeper: all sinks failed for stale entry"
                );
                let current_retries = entry.retries;
                let mut active: outbox_entity::ActiveModel = entry.into();
                active.retries = Set(current_retries + 1);
                if current_retries + 1 >= 10 {
                    active.status = Set("failed".to_string());
                    sweeper_entries_failed().inc();
                    warn!("sweeper: outbox entry exceeded max reconciliation retries, marking failed");
                }
                if let Err(db_err) = active.update(&state.db).await {
                    error!(
                        error = %db_err,
                        "sweeper: failed to update outbox entry retry counter"
                    );
                }
            }
            DeliveryResult::PartialFailure { succeeded, failed } => {
                warn!(
                    succeeded,
                    failed,
                    "sweeper: partial sink failure — will retry on next sweep"
                );
                let current_retries = entry.retries;
                let mut active: outbox_entity::ActiveModel = entry.into();
                active.retries = Set(current_retries + 1);
                if current_retries + 1 >= 10 {
                    active.status = Set("failed".to_string());
                    sweeper_entries_failed().inc();
                    warn!("sweeper: outbox entry exceeded max reconciliation retries, marking failed");
                }
                if let Err(db_err) = active.update(&state.db).await {
                    error!(
                        error = %db_err,
                        "sweeper: failed to update outbox entry retry counter"
                    );
                }
            }
        }
    }

    Ok(count)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Mark the DB outbox entry as flushed for observability.
///
/// Returns Ok(()) even if no pending entry exists (idempotent — handles
/// races between outbox worker and sweeper, or duplicate NATS redelivery).
async fn mark_flushed(
    db: &DatabaseConnection,
    assessment_id: uuid::Uuid,
) -> Result<(), sea_orm::DbErr> {
    let record = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .filter(outbox_entity::Column::Status.eq("pending"))
        .one(db)
        .await?;

    match record {
        Some(record) => {
            let mut active: outbox_entity::ActiveModel = record.into();
            active.status = Set("flushed".to_string());
            active.flushed_at = Set(Some(Utc::now().into()));
            active.update(db).await?;
        }
        None => {
            debug!(
                %assessment_id,
                "mark_flushed: no pending entry found (already flushed or not yet created)"
            );
        }
    }

    Ok(())
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
