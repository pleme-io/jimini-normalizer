use jimini_normalizer::envelope::{AuditPayload, OutboxEnvelope};
use jimini_normalizer::model::unified::{AssessmentMetadata, Score, UnifiedAssessment};

fn sample_envelope() -> OutboxEnvelope {
    OutboxEnvelope {
        assessment: UnifiedAssessment {
            id: uuid::Uuid::new_v4(),
            patient_id: "test-patient-hash".to_string(),
            assessment_date: chrono::Utc::now(),
            assessment_type: "PHQ-9".to_string(),
            scores: vec![Score {
                dimension: "depression".to_string(),
                value: 15.0,
                scale: "0-27".to_string(),
            }],
            metadata: AssessmentMetadata {
                source_provider: "provider_a".to_string(),
                source_format: "json".to_string(),
                ingested_at: chrono::Utc::now(),
                version: "1.0".to_string(),
            },
        },
        audit: AuditPayload {
            input_hash: "abc123".to_string(),
            input_size_bytes: 256,
            transformations: serde_json::json!([]),
            duration_us: 1234,
            provider: "provider_a".to_string(),
            source_format: "json".to_string(),
            score_count: 1,
        },
    }
}

#[test]
fn test_outbox_envelope_serialization_roundtrip() {
    let envelope = sample_envelope();

    let json = serde_json::to_string(&envelope).expect("serialize");
    let deserialized: OutboxEnvelope = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(deserialized.assessment.id, envelope.assessment.id);
    assert_eq!(
        deserialized.assessment.patient_id,
        envelope.assessment.patient_id
    );
    assert_eq!(
        deserialized.assessment.assessment_type,
        envelope.assessment.assessment_type
    );
    assert_eq!(deserialized.audit.input_hash, envelope.audit.input_hash);
    assert_eq!(deserialized.audit.provider, envelope.audit.provider);
    assert_eq!(deserialized.audit.score_count, envelope.audit.score_count);
}

#[test]
fn test_outbox_envelope_backward_compat_assessment_only() {
    // Legacy format: only an assessment (no audit field)
    // The OutboxEnvelope requires audit, so this should fail to deserialize —
    // confirming the outbox worker's legacy fallback path is needed.
    let assessment = sample_envelope().assessment;
    let json = serde_json::to_string(&assessment).expect("serialize");

    let result = serde_json::from_str::<OutboxEnvelope>(&json);
    assert!(
        result.is_err(),
        "assessment-only payload should not deserialize as OutboxEnvelope"
    );
}

#[test]
fn test_outbox_envelope_value_roundtrip() {
    let envelope = sample_envelope();

    let value = serde_json::to_value(&envelope).expect("to_value");
    let deserialized: OutboxEnvelope = serde_json::from_value(value).expect("from_value");

    assert_eq!(deserialized.assessment.id, envelope.assessment.id);
    assert_eq!(
        deserialized.audit.input_size_bytes,
        envelope.audit.input_size_bytes
    );
}

#[test]
fn test_audit_payload_all_fields_present() {
    let audit = AuditPayload {
        input_hash: "deadbeef".to_string(),
        input_size_bytes: 1024,
        transformations: serde_json::json!({"steps": []}),
        duration_us: 5000,
        provider: "test".to_string(),
        source_format: "csv".to_string(),
        score_count: 3,
    };

    let json = serde_json::to_value(&audit).expect("serialize");
    assert_eq!(json["input_hash"], "deadbeef");
    assert_eq!(json["input_size_bytes"], 1024);
    assert_eq!(json["duration_us"], 5000);
    assert_eq!(json["score_count"], 3);
}
