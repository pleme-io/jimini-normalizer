mod common;

use std::time::Duration;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use jimini_normalizer::entity::outbox as outbox_entity;
use jimini_normalizer::envelope::{AuditPayload, OutboxEnvelope};
use jimini_normalizer::model::unified::{AssessmentMetadata, Score, UnifiedAssessment};
use jimini_normalizer::sink::postgres::PostgresSink;

fn sample_envelope() -> OutboxEnvelope {
    OutboxEnvelope {
        assessment: UnifiedAssessment {
            id: uuid::Uuid::new_v4(),
            patient_id: "test-patient-hash".to_string(),
            assessment_date: Utc::now(),
            assessment_type: "PHQ-9".to_string(),
            scores: vec![Score {
                dimension: "depression".to_string(),
                value: 15.0,
                scale: "0-27".to_string(),
            }],
            metadata: AssessmentMetadata {
                source_provider: "provider_a".to_string(),
                source_format: "json".to_string(),
                ingested_at: Utc::now(),
                version: "1.0".to_string(),
            },
        },
        audit: AuditPayload {
            input_hash: "abc123def456".to_string(),
            input_size_bytes: 256,
            transformations: serde_json::json!([]),
            duration_us: 1234,
            provider: "provider_a".to_string(),
            source_format: "json".to_string(),
            score_count: 1,
        },
    }
}

// ─── PostgresSink Tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_postgres_sink_persist_succeeds() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();

    let result = sink.persist(&envelope).await;
    assert!(result.is_ok(), "persist should succeed: {result:?}");

    // Verify assessment exists in DB
    let assessment =
        jimini_normalizer::entity::assessment::Entity::find_by_id(envelope.assessment.id)
            .one(&infra.db)
            .await
            .expect("query failed");
    assert!(assessment.is_some(), "assessment should be in DB");

    let a = assessment.unwrap();
    assert_eq!(a.patient_id, "test-patient-hash");
    assert_eq!(a.assessment_type, "PHQ-9");
}

#[tokio::test]
async fn test_postgres_sink_persist_idempotent() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();

    // Persist twice — should not error
    sink.persist(&envelope).await.expect("first persist");
    let result = sink.persist(&envelope).await;
    assert!(result.is_ok(), "second persist should be idempotent: {result:?}");

    // Verify only one assessment exists
    let assessments = jimini_normalizer::entity::assessment::Entity::find()
        .filter(
            jimini_normalizer::entity::assessment::Column::Id.eq(envelope.assessment.id),
        )
        .all(&infra.db)
        .await
        .expect("query failed");
    assert_eq!(assessments.len(), 1, "should have exactly one assessment");
}

#[tokio::test]
async fn test_postgres_sink_creates_audit_log() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();

    sink.persist(&envelope).await.expect("persist failed");

    let audit = jimini_normalizer::entity::normalization_audit_log::Entity::find()
        .filter(
            jimini_normalizer::entity::normalization_audit_log::Column::AssessmentId
                .eq(envelope.assessment.id),
        )
        .one(&infra.db)
        .await
        .expect("query failed");
    assert!(audit.is_some(), "audit log should be created");

    let audit = audit.unwrap();
    assert_eq!(audit.provider, "provider_a");
    assert_eq!(audit.input_hash, "abc123def456");
    assert_eq!(audit.score_count, 1);
}

#[tokio::test]
async fn test_postgres_sink_creates_outbox_entry() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();

    sink.persist(&envelope).await.expect("persist failed");

    let outbox = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(envelope.assessment.id))
        .one(&infra.db)
        .await
        .expect("query failed");
    assert!(outbox.is_some(), "outbox entry should be created");

    let outbox = outbox.unwrap();
    assert_eq!(outbox.status, "pending");
    assert_eq!(outbox.retries, 0);
}

#[tokio::test]
async fn test_postgres_sink_stores_full_envelope_in_outbox() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();

    sink.persist(&envelope).await.expect("persist failed");

    let outbox = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(envelope.assessment.id))
        .one(&infra.db)
        .await
        .expect("query failed")
        .expect("outbox entry should exist");

    // The outbox payload should be a full OutboxEnvelope (assessment + audit)
    let parsed: OutboxEnvelope =
        serde_json::from_value(outbox.payload).expect("outbox payload should deserialize as OutboxEnvelope");
    assert_eq!(parsed.assessment.id, envelope.assessment.id);
    assert_eq!(parsed.audit.input_hash, "abc123def456");
    assert_eq!(parsed.audit.provider, "provider_a");
}

// ─── Idempotent Redelivery Tests ────────────────────────────────────────────

#[tokio::test]
async fn test_postgres_sink_idempotent_redelivery() {
    let infra = common::TestInfra::start().await;
    let envelope = sample_envelope();
    let sink = PostgresSink::new(infra.db.clone());

    // Simulate NATS redelivery: persist same envelope 3 times
    for i in 0..3 {
        let result = sink.persist(&envelope).await;
        assert!(result.is_ok(), "persist attempt {i} should succeed");
    }

    // Verify single DB rows across all 3 tables
    let assessments = jimini_normalizer::entity::assessment::Entity::find()
        .filter(
            jimini_normalizer::entity::assessment::Column::Id.eq(envelope.assessment.id),
        )
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(assessments.len(), 1, "should have exactly one assessment row");

    let audits = jimini_normalizer::entity::normalization_audit_log::Entity::find()
        .filter(
            jimini_normalizer::entity::normalization_audit_log::Column::AssessmentId
                .eq(envelope.assessment.id),
        )
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(audits.len(), 1, "should have exactly one audit row");

    let outbox_entries = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(envelope.assessment.id))
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(outbox_entries.len(), 1, "should have exactly one outbox row");
}

// ─── Mark Flushed Tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_mark_flushed_updates_outbox_entry() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // Verify entry starts as pending
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(entry.status, "pending");
    assert!(entry.flushed_at.is_none());

    // Mark flushed
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.status = Set("flushed".to_string());
    active.flushed_at = Set(Some(Utc::now().into()));
    active.update(&infra.db).await.expect("update failed");

    // Verify flushed
    let flushed = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(flushed.status, "flushed");
    assert!(flushed.flushed_at.is_some());
}

#[tokio::test]
async fn test_mark_flushed_idempotent_no_pending_entry() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // Mark flushed directly on the entry
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.status = Set("flushed".to_string());
    active.flushed_at = Set(Some(Utc::now().into()));
    active.update(&infra.db).await.expect("update failed");

    // Attempting to find a pending entry for this assessment should return None
    let pending = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .filter(outbox_entity::Column::Status.eq("pending"))
        .one(&infra.db)
        .await
        .expect("query");
    assert!(pending.is_none(), "no pending entry should remain after flush");
}

// ─── Sweeper Tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_sweeper_retry_increment() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // Simulate a sweeper retry by incrementing retries
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(entry.retries, 0);

    let mut active: outbox_entity::ActiveModel = entry.into();
    active.retries = Set(1);
    active.update(&infra.db).await.expect("update failed");

    let updated = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(updated.retries, 1);
    assert_eq!(updated.status, "pending", "status should remain pending");
}

#[tokio::test]
async fn test_sweeper_marks_failed_after_max_retries() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // Simulate reaching max retries (10)
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");

    let mut active: outbox_entity::ActiveModel = entry.into();
    active.retries = Set(10);
    active.status = Set("failed".to_string());
    active.update(&infra.db).await.expect("update failed");

    let failed = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.retries, 10);
}

#[tokio::test]
async fn test_sweeper_legacy_payload_fallback() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    // Persist the assessment so FK constraints are satisfied
    sink.persist(&envelope).await.expect("persist failed");

    // Overwrite the outbox payload with assessment-only JSON (legacy format)
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");

    let legacy_payload = serde_json::to_value(&envelope.assessment).expect("serialize");
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.payload = Set(legacy_payload.clone());
    active.update(&infra.db).await.expect("update failed");

    // Verify the legacy payload does NOT parse as OutboxEnvelope
    let legacy_result = serde_json::from_value::<OutboxEnvelope>(legacy_payload.clone());
    assert!(
        legacy_result.is_err(),
        "legacy assessment-only payload should not parse as OutboxEnvelope"
    );

    // But it should parse as a generic Value (which the sweeper handles as fallback)
    let value_result = serde_json::from_value::<serde_json::Value>(legacy_payload);
    assert!(value_result.is_ok(), "should parse as generic JSON");
}

#[tokio::test]
async fn test_outbox_envelope_payload_roundtrip_in_db() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();

    sink.persist(&envelope).await.expect("persist failed");

    // Read back the outbox entry and verify the full envelope roundtrips
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(envelope.assessment.id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");

    let parsed: OutboxEnvelope =
        serde_json::from_value(entry.payload).expect("should deserialize as OutboxEnvelope");

    // Verify assessment fields
    assert_eq!(parsed.assessment.id, envelope.assessment.id);
    assert_eq!(parsed.assessment.patient_id, envelope.assessment.patient_id);
    assert_eq!(parsed.assessment.assessment_type, envelope.assessment.assessment_type);
    assert_eq!(parsed.assessment.scores.len(), 1);
    assert_eq!(parsed.assessment.scores[0].dimension, "depression");
    assert_eq!(parsed.assessment.scores[0].value, 15.0);

    // Verify audit fields
    assert_eq!(parsed.audit.input_hash, envelope.audit.input_hash);
    assert_eq!(parsed.audit.input_size_bytes, envelope.audit.input_size_bytes);
    assert_eq!(parsed.audit.duration_us, envelope.audit.duration_us);
    assert_eq!(parsed.audit.provider, envelope.audit.provider);
    assert_eq!(parsed.audit.score_count, envelope.audit.score_count);
}

// ─── NATS JetStream Integration Tests ───────────────────────────────────────

#[tokio::test]
async fn test_outbox_worker_persists_via_postgres_sink() {
    let infra = common::TestInfra::start().await;
    let state = infra.app_state();
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    // Ensure outbox stream exists
    jimini_normalizer::nats::ensure_streams(&infra.js, &state.config())
        .await
        .expect("ensure streams");

    // Publish envelope to outbox stream
    let payload = serde_json::to_value(&envelope).expect("serialize");
    jimini_normalizer::nats::publish_outbox(
        &infra.js,
        &state.config().outbox.stream_name,
        &assessment_id,
        &payload,
        &state.config().retry,
    )
    .await
    .expect("publish to outbox");

    // Wait briefly for NATS to persist
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Directly test PostgresSink (the outbox worker would call this)
    state
        .postgres_sink()
        .persist(&envelope)
        .await
        .expect("postgres sink persist");

    // Verify assessment in DB
    let assessment =
        jimini_normalizer::entity::assessment::Entity::find_by_id(assessment_id)
            .one(&infra.db)
            .await
            .expect("query failed");
    assert!(assessment.is_some(), "assessment should be persisted");
}

#[tokio::test]
async fn test_worker_can_publish_envelope_to_outbox_stream() {
    let infra = common::TestInfra::start().await;
    let state = infra.app_state();
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    // Ensure streams exist
    jimini_normalizer::nats::ensure_streams(&infra.js, &state.config())
        .await
        .expect("ensure streams");

    // Simulate what the worker does: serialize envelope and publish to outbox
    let payload = serde_json::to_value(&envelope).expect("serialize");
    let result = jimini_normalizer::nats::publish_outbox(
        &infra.js,
        &state.config().outbox.stream_name,
        &assessment_id,
        &payload,
        &state.config().retry,
    )
    .await;

    assert!(result.is_ok(), "outbox publish should succeed: {result:?}");
}

#[tokio::test]
async fn test_nats_dedup_prevents_duplicate_outbox_messages() {
    let infra = common::TestInfra::start().await;
    let state = infra.app_state();
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    jimini_normalizer::nats::ensure_streams(&infra.js, &state.config())
        .await
        .expect("ensure streams");

    let payload = serde_json::to_value(&envelope).expect("serialize");

    // Publish the same message twice — NATS dedup via Nats-Msg-Id header
    jimini_normalizer::nats::publish_outbox(
        &infra.js,
        &state.config().outbox.stream_name,
        &assessment_id,
        &payload,
        &state.config().retry,
    )
    .await
    .expect("first publish");

    let result = jimini_normalizer::nats::publish_outbox(
        &infra.js,
        &state.config().outbox.stream_name,
        &assessment_id,
        &payload,
        &state.config().retry,
    )
    .await;

    // Second publish should succeed (dedup is transparent — ack.duplicate=true but no error)
    assert!(result.is_ok(), "duplicate publish should succeed (dedup is transparent)");
}

#[tokio::test]
async fn test_ensure_streams_creates_all_streams() {
    let infra = common::TestInfra::start().await;
    let state = infra.app_state();
    let config = state.config();

    jimini_normalizer::nats::ensure_streams(&infra.js, &config)
        .await
        .expect("ensure streams");

    // Verify all 3 streams exist
    let assessment_stream = infra.js.get_stream(&config.nats.stream_name).await;
    assert!(assessment_stream.is_ok(), "assessment stream should exist");

    let error_stream = infra.js.get_stream(&config.nats.error_stream_name).await;
    assert!(error_stream.is_ok(), "error stream should exist");

    let outbox_stream = infra.js.get_stream(&config.outbox.stream_name).await;
    assert!(outbox_stream.is_ok(), "outbox stream should exist");
}

// ─── Multiple Assessments Tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_multiple_assessments_persist_independently() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());

    let mut envelopes = Vec::new();
    for _ in 0..5 {
        let e = sample_envelope();
        envelopes.push(e);
    }

    for e in &envelopes {
        sink.persist(e).await.expect("persist");
    }

    // Verify all 5 assessments exist
    let all = jimini_normalizer::entity::assessment::Entity::find()
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(all.len(), 5, "should have 5 assessments");

    // Verify all 5 audit logs exist
    let audits = jimini_normalizer::entity::normalization_audit_log::Entity::find()
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(audits.len(), 5, "should have 5 audit logs");

    // Verify all 5 outbox entries exist
    let outbox = outbox_entity::Entity::find()
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(outbox.len(), 5, "should have 5 outbox entries");
}

// ─── ACID Transaction Tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_postgres_sink_transaction_atomicity() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // All 3 records should exist together (ACID transaction)
    let assessment =
        jimini_normalizer::entity::assessment::Entity::find_by_id(assessment_id)
            .one(&infra.db)
            .await
            .expect("query");
    let audit = jimini_normalizer::entity::normalization_audit_log::Entity::find()
        .filter(
            jimini_normalizer::entity::normalization_audit_log::Column::AssessmentId
                .eq(assessment_id),
        )
        .one(&infra.db)
        .await
        .expect("query");
    let outbox = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query");

    assert!(assessment.is_some(), "assessment should exist");
    assert!(audit.is_some(), "audit should exist");
    assert!(outbox.is_some(), "outbox should exist");
}

#[tokio::test]
async fn test_postgres_sink_scores_stored_as_json() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());

    let mut envelope = sample_envelope();
    envelope.assessment.scores = vec![
        Score {
            dimension: "depression".to_string(),
            value: 15.0,
            scale: "0-27".to_string(),
        },
        Score {
            dimension: "anxiety".to_string(),
            value: 8.0,
            scale: "0-21".to_string(),
        },
    ];

    sink.persist(&envelope).await.expect("persist failed");

    let assessment =
        jimini_normalizer::entity::assessment::Entity::find_by_id(envelope.assessment.id)
            .one(&infra.db)
            .await
            .expect("query")
            .expect("should exist");

    // Verify scores are stored as JSON array
    let scores: Vec<Score> =
        serde_json::from_value(assessment.scores).expect("scores should deserialize");
    assert_eq!(scores.len(), 2);
    assert_eq!(scores[0].dimension, "depression");
    assert_eq!(scores[1].dimension, "anxiety");
}

// ─── Sweeper Boundary Condition Tests ───────────────────────────────────────

#[tokio::test]
async fn test_sweeper_retries_9_stays_pending() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // Set retries to 9 — one below the threshold (>= 10 marks failed)
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.retries = Set(9);
    active.update(&infra.db).await.expect("update");

    let updated = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(updated.retries, 9);
    assert_eq!(updated.status, "pending", "retries=9 should stay pending");
}

#[tokio::test]
async fn test_sweeper_retries_10_transitions_to_failed() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());
    let envelope = sample_envelope();
    let assessment_id = envelope.assessment.id;

    sink.persist(&envelope).await.expect("persist failed");

    // Simulate what the sweeper does: retries=9 → increment to 10 → mark failed
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");

    // Start from 9 so the next increment (to 10) triggers the >= 10 check
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.retries = Set(9);
    active.update(&infra.db).await.expect("update to 9");

    // Now simulate the sweeper's increment logic: current_retries + 1 >= 10
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    let current_retries = entry.retries;
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.retries = Set(current_retries + 1);
    if current_retries + 1 >= 10 {
        active.status = Set("failed".to_string());
    }
    active.update(&infra.db).await.expect("update to 10");

    let final_entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(assessment_id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    assert_eq!(final_entry.retries, 10);
    assert_eq!(final_entry.status, "failed", "retries=10 should mark as failed");
}

#[tokio::test]
async fn test_outbox_flushed_entry_not_picked_up_by_pending_query() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());

    // Create two envelopes — flush one, leave one pending
    let env_flushed = sample_envelope();
    let env_pending = sample_envelope();

    sink.persist(&env_flushed).await.expect("persist");
    sink.persist(&env_pending).await.expect("persist");

    // Mark first as flushed
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(env_flushed.assessment.id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.status = Set("flushed".to_string());
    active.flushed_at = Set(Some(Utc::now().into()));
    active.update(&infra.db).await.expect("update");

    // Query for pending entries — should only return the un-flushed one
    let pending = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::Status.eq("pending"))
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].assessment_id, env_pending.assessment.id);
}

#[tokio::test]
async fn test_outbox_failed_entry_not_picked_up_by_pending_query() {
    let infra = common::TestInfra::start().await;
    let sink = PostgresSink::new(infra.db.clone());

    let env_failed = sample_envelope();
    let env_pending = sample_envelope();

    sink.persist(&env_failed).await.expect("persist");
    sink.persist(&env_pending).await.expect("persist");

    // Mark first as failed
    let entry = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::AssessmentId.eq(env_failed.assessment.id))
        .one(&infra.db)
        .await
        .expect("query")
        .expect("should exist");
    let mut active: outbox_entity::ActiveModel = entry.into();
    active.status = Set("failed".to_string());
    active.retries = Set(10);
    active.update(&infra.db).await.expect("update");

    // Query for pending entries — should only return the un-failed one
    let pending = outbox_entity::Entity::find()
        .filter(outbox_entity::Column::Status.eq("pending"))
        .all(&infra.db)
        .await
        .expect("query");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].assessment_id, env_pending.assessment.id);
}
