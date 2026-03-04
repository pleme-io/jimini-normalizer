use std::time::{Duration, Instant};

use chrono::Utc;
use sea_orm::{DatabaseConnection, EntityTrait, Set, TransactionTrait};
use tracing::{debug, warn};

use crate::entity::{assessment as assessment_entity, normalization_audit_log};
use crate::envelope::OutboxEnvelope;
use crate::observability::{assessments_stored, postgres_sink_duration, postgres_sink_errors, postgres_sink_retries};

/// Error type for PostgresSink operations.
#[derive(Debug, thiserror::Error)]
pub enum PostgresSinkError {
    #[error("database error: {0}")]
    Database(#[from] sea_orm::DbErr),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Primary sink that persists assessments to PostgreSQL.
///
/// Runs a single ACID transaction:
/// 1. INSERT assessment (ON CONFLICT DO NOTHING)
/// 2. INSERT audit_log (ON CONFLICT DO NOTHING)
/// 3. INSERT outbox entry (ON CONFLICT DO NOTHING)
/// 4. COMMIT
///
/// All inserts are idempotent — safe for NATS redelivery.
/// Transient DB errors are retried with exponential backoff before
/// surfacing the error to the caller for NATS NAK.
pub struct PostgresSink {
    db: DatabaseConnection,
}

impl PostgresSink {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Persist an assessment envelope with retry on transient DB errors.
    ///
    /// Retries up to 3 times with exponential backoff (100ms, 200ms, 400ms)
    /// on transient errors (connection refused, timeout, serialization failure).
    /// Returns Ok(()) on success or if the assessment already exists (idempotent).
    pub async fn persist(&self, envelope: &OutboxEnvelope) -> Result<(), PostgresSinkError> {
        let max_attempts = 3u32;
        let mut attempt = 0u32;
        let start = Instant::now();

        loop {
            attempt += 1;
            match self.persist_inner(envelope).await {
                Ok(()) => {
                    postgres_sink_duration()
                        .with_label_values(&[])
                        .observe(start.elapsed().as_secs_f64());
                    return Ok(());
                }
                Err(e) if attempt < max_attempts && is_transient(&e) => {
                    postgres_sink_retries().inc();
                    let delay = Duration::from_millis(100 * 2u64.pow(attempt - 1));
                    warn!(
                        attempt,
                        max = max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        assessment_id = %envelope.assessment.id,
                        "PostgresSink: transient DB error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    postgres_sink_duration()
                        .with_label_values(&[])
                        .observe(start.elapsed().as_secs_f64());
                    let error_type = if is_transient(&e) { "transient" } else { "permanent" };
                    postgres_sink_errors()
                        .with_label_values(&[error_type])
                        .inc();
                    return Err(e);
                }
            }
        }
    }

    /// Inner persist — single attempt, no retry.
    async fn persist_inner(&self, envelope: &OutboxEnvelope) -> Result<(), PostgresSinkError> {
        let a = &envelope.assessment;
        let audit = &envelope.audit;

        let txn = self.db.begin().await?;

        // 1. INSERT assessment — idempotent: ON CONFLICT (id) DO NOTHING
        let scores_json = serde_json::to_value(&a.scores)?;
        let entity = assessment_entity::ActiveModel {
            id: Set(a.id),
            patient_id: Set(a.patient_id.clone()),
            assessment_date: Set(a.assessment_date),
            assessment_type: Set(a.assessment_type.clone()),
            scores: Set(scores_json),
            source_provider: Set(a.metadata.source_provider.clone()),
            source_format: Set(a.metadata.source_format.clone()),
            ingested_at: Set(a.metadata.ingested_at),
            version: Set(a.metadata.version.clone()),
            created_at: Set(Utc::now().into()),
        };
        assessment_entity::Entity::insert(entity)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(assessment_entity::Column::Id)
                    .do_nothing()
                    .to_owned(),
            )
            .do_nothing()
            .exec(&txn)
            .await?;

        // 2. INSERT audit_log — idempotent: ON CONFLICT (assessment_id) DO NOTHING
        let audit_entry = normalization_audit_log::ActiveModel {
            id: Set(uuid::Uuid::new_v4()),
            assessment_id: Set(a.id),
            provider: Set(audit.provider.clone()),
            source_format: Set(audit.source_format.clone()),
            input_hash: Set(audit.input_hash.clone()),
            input_size_bytes: Set(audit.input_size_bytes),
            score_count: Set(audit.score_count),
            transformations: Set(audit.transformations.clone()),
            duration_us: Set(audit.duration_us),
            created_at: Set(Utc::now().into()),
        };
        normalization_audit_log::Entity::insert(audit_entry)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(
                    normalization_audit_log::Column::AssessmentId,
                )
                .do_nothing()
                .to_owned(),
            )
            .do_nothing()
            .exec(&txn)
            .await?;

        // 3. INSERT outbox entry — stores full envelope for sweeper compatibility
        let outbox_payload = serde_json::to_value(envelope)?;
        crate::outbox::enqueue_in_txn(&txn, a.id, outbox_payload).await?;

        txn.commit().await?;

        assessments_stored().inc();
        debug!(assessment_id = %a.id, "PostgresSink: persisted assessment");
        Ok(())
    }
}

/// Detect transient DB errors safe to retry.
///
/// Serialization (serde_json) errors are never transient — retrying won't help.
/// Only DB errors are inspected via string matching for known transient patterns.
fn is_transient(err: &PostgresSinkError) -> bool {
    match err {
        PostgresSinkError::Serialization(_) => false,
        PostgresSinkError::Database(db_err) => {
            let msg = db_err.to_string().to_lowercase();
            msg.contains("connection refused")
                || msg.contains("connection reset")
                || msg.contains("broken pipe")
                || msg.contains("timed out")
                || msg.contains("timeout")
                || msg.contains("could not serialize access")
                || msg.contains("too many connections")
                || msg.contains("pool timed out")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::DbErr;

    #[test]
    fn test_serialization_error_is_never_transient() {
        // Even if the serde message happens to contain "timeout", it should NOT be transient
        let bad_json: Result<String, _> = serde_json::from_str("not json");
        let err = PostgresSinkError::Serialization(bad_json.unwrap_err());
        assert!(!is_transient(&err), "serialization errors must never be transient");
    }

    #[test]
    fn test_connection_refused_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("Connection refused".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_connection_reset_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("Connection reset by peer".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_broken_pipe_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("Broken pipe".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_timeout_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("query timed out".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_pool_timed_out_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("pool timed out waiting for connection".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_serialization_failure_postgres_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("could not serialize access due to concurrent update".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_too_many_connections_is_transient() {
        let err = PostgresSinkError::Database(DbErr::Conn(
            sea_orm::RuntimeErr::Internal("FATAL: too many connections for role".to_string()),
        ));
        assert!(is_transient(&err));
    }

    #[test]
    fn test_unique_violation_is_not_transient() {
        let err = PostgresSinkError::Database(DbErr::Exec(
            sea_orm::RuntimeErr::Internal("duplicate key value violates unique constraint".to_string()),
        ));
        assert!(!is_transient(&err), "unique violations are permanent errors");
    }

    #[test]
    fn test_syntax_error_is_not_transient() {
        let err = PostgresSinkError::Database(DbErr::Exec(
            sea_orm::RuntimeErr::Internal("syntax error at or near SELECT".to_string()),
        ));
        assert!(!is_transient(&err), "SQL syntax errors are permanent");
    }
}
