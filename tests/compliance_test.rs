//! HIPAA/SOC2 Compliance Tests
//!
//! Verifies that PII/PHI scrubbing is correctly applied across all data paths:
//! - Pipeline output (patient_id scrubbing)
//! - Error payloads (no raw_input_preview)
//! - PII config validation (mandatory, no defaults)
//! - Internal vs external PII boundary enforcement
//! - HMAC-SHA256 salted hashing (prevents rainbow table attacks)

use std::collections::HashMap;

use jimini_normalizer::pii::{
    self, FieldRule, InternalPiiPolicy, PiiConfig, ScrubStrategy,
    process_patient_id_for_storage, process_raw_input, scrub_patient_id, scrub_raw_input,
};
use jimini_normalizer::pipeline::NormalizationPipeline;
use jimini_normalizer::provider::provider_a::ProviderA;
use jimini_normalizer::provider::provider_b::ProviderB;
use jimini_normalizer::provider::provider_c::ProviderC;

const TEST_SALT: &str = "test-salt-at-least-32-characters-long-for-tests";

/// Build a PII config for testing with full scrubbing (no internal PII).
fn strict_pii_config() -> PiiConfig {
    PiiConfig {
        enabled: true,
        patient_id_strategy: ScrubStrategy::Hash,
        raw_input_strategy: ScrubStrategy::Hash,
        hash_salt: TEST_SALT.to_string(),
        provider_fields: HashMap::from([
            ("provider_a".into(), vec![
                FieldRule { field: "name".into(), strategy: ScrubStrategy::Redact },
                FieldRule { field: "dob".into(), strategy: ScrubStrategy::Redact },
            ]),
            ("provider_b".into(), vec![
                FieldRule { field: "patient_name".into(), strategy: ScrubStrategy::Redact },
            ]),
        ]),
        internal: InternalPiiPolicy {
            allow_internal_pii: false,
            store_patient_id_plaintext: false,
            store_raw_for_replay: false,
        },
    }
}

/// Build a PII config that allows internal PII (for replay scenarios).
fn permissive_internal_pii_config() -> PiiConfig {
    PiiConfig {
        enabled: true,
        patient_id_strategy: ScrubStrategy::Hash,
        raw_input_strategy: ScrubStrategy::Hash,
        hash_salt: TEST_SALT.to_string(),
        provider_fields: HashMap::from([
            ("provider_a".into(), vec![
                FieldRule { field: "name".into(), strategy: ScrubStrategy::Redact },
                FieldRule { field: "dob".into(), strategy: ScrubStrategy::Redact },
            ]),
            ("provider_b".into(), vec![
                FieldRule { field: "patient_name".into(), strategy: ScrubStrategy::Redact },
            ]),
        ]),
        internal: InternalPiiPolicy {
            allow_internal_pii: true,
            store_patient_id_plaintext: true,
            store_raw_for_replay: true,
        },
    }
}

// =============================================================================
// PII Config Validation
// =============================================================================

#[test]
#[should_panic(expected = "pii.enabled must be explicitly set to true")]
fn service_refuses_to_start_without_pii_config() {
    let config = PiiConfig {
        enabled: false,
        patient_id_strategy: ScrubStrategy::Hash,
        raw_input_strategy: ScrubStrategy::Hash,
        hash_salt: TEST_SALT.to_string(),
        provider_fields: HashMap::new(),
        internal: InternalPiiPolicy {
            allow_internal_pii: false,
            store_patient_id_plaintext: false,
            store_raw_for_replay: false,
        },
    };
    config.validate_or_panic();
}

#[test]
#[should_panic(expected = "pii.hash_salt must be at least 32 characters")]
fn service_refuses_to_start_without_hash_salt() {
    let config = PiiConfig {
        enabled: true,
        patient_id_strategy: ScrubStrategy::Hash,
        raw_input_strategy: ScrubStrategy::Hash,
        hash_salt: "too-short".to_string(),
        provider_fields: HashMap::new(),
        internal: InternalPiiPolicy {
            allow_internal_pii: false,
            store_patient_id_plaintext: false,
            store_raw_for_replay: false,
        },
    };
    config.validate_or_panic();
}

#[test]
#[should_panic(expected = "store_patient_id_plaintext requires")]
fn service_refuses_inconsistent_patient_id_config() {
    let config = PiiConfig {
        enabled: true,
        patient_id_strategy: ScrubStrategy::Hash,
        raw_input_strategy: ScrubStrategy::Hash,
        hash_salt: TEST_SALT.to_string(),
        provider_fields: HashMap::new(),
        internal: InternalPiiPolicy {
            allow_internal_pii: false,
            store_patient_id_plaintext: true,
            store_raw_for_replay: false,
        },
    };
    config.validate_or_panic();
}

#[test]
#[should_panic(expected = "store_raw_for_replay requires")]
fn service_refuses_inconsistent_raw_input_config() {
    let config = PiiConfig {
        enabled: true,
        patient_id_strategy: ScrubStrategy::Hash,
        raw_input_strategy: ScrubStrategy::Hash,
        hash_salt: TEST_SALT.to_string(),
        provider_fields: HashMap::new(),
        internal: InternalPiiPolicy {
            allow_internal_pii: false,
            store_patient_id_plaintext: false,
            store_raw_for_replay: true,
        },
    };
    config.validate_or_panic();
}

#[test]
fn pii_config_with_all_options_validates() {
    let config = permissive_internal_pii_config();
    config.validate_or_panic(); // should not panic
}

// =============================================================================
// Pipeline PII Scrubbing — Strict Mode (no internal PII)
// =============================================================================

#[test]
fn pipeline_scrubs_patient_id_with_strict_pii() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderA, input, Some(&config)).unwrap();

    for assessment in &result.assessments {
        assert!(
            assessment.patient_id.starts_with("sha256:"),
            "patient_id must be hashed in strict mode, got: {}",
            assessment.patient_id
        );
        assert!(
            !assessment.patient_id.contains("P123"),
            "plaintext patient_id must not appear"
        );
    }
}

#[test]
fn pipeline_scrubs_patient_id_provider_b_strict() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderB, input, Some(&config)).unwrap();

    for assessment in &result.assessments {
        assert!(assessment.patient_id.starts_with("sha256:"));
        assert!(!assessment.patient_id.contains("P123"));
    }
}

#[test]
fn pipeline_scrubs_patient_id_provider_c_strict() {
    let input = include_bytes!("../fixtures/provider_c.csv");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderC, input, Some(&config)).unwrap();

    for assessment in &result.assessments {
        assert!(assessment.patient_id.starts_with("sha256:"));
        assert!(!assessment.patient_id.contains("P123"));
    }
}

// =============================================================================
// Pipeline PII Scrubbing — Permissive Internal Mode
// =============================================================================

#[test]
fn pipeline_preserves_patient_id_with_internal_pii() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let config = permissive_internal_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderA, input, Some(&config)).unwrap();

    for assessment in &result.assessments {
        assert_eq!(
            assessment.patient_id, "P123",
            "patient_id should be preserved for internal storage when configured"
        );
    }
}

#[test]
fn api_response_scrubs_even_when_internal_preserves() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let config = permissive_internal_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderA, input, Some(&config)).unwrap();

    // Simulate API response scrubbing (what assessments handler does)
    for assessment in &result.assessments {
        let scrubbed = scrub_patient_id(&assessment.patient_id, &config.patient_id_strategy, &config.hash_salt);
        assert!(scrubbed.starts_with("sha256:"), "API response must always scrub");
        assert!(!scrubbed.contains("P123"));
    }
}

// =============================================================================
// Raw Input Handling
// =============================================================================

#[test]
fn raw_input_scrubbed_in_strict_mode() {
    let raw = b"{\"patient\": {\"id\": \"P123\", \"name\": \"Jane Doe\"}}";
    let config = strict_pii_config();
    let result = process_raw_input(raw, &config);

    assert!(result.starts_with("sha256:"), "raw input must be hashed in strict mode");
    assert!(!result.contains("Jane Doe"), "PHI must not appear in scrubbed output");
    assert!(!result.contains("P123"), "PII must not appear in scrubbed output");
}

#[test]
fn raw_input_preserved_when_internal_allows() {
    let raw = b"{\"patient\": {\"id\": \"P123\"}}";
    let config = permissive_internal_pii_config();
    let result = process_raw_input(raw, &config);

    assert_eq!(result, "{\"patient\": {\"id\": \"P123\"}}");
}

#[test]
fn raw_input_redact_strategy() {
    let raw = b"sensitive data";
    let result = scrub_raw_input(raw, &ScrubStrategy::Redact, TEST_SALT);
    assert_eq!(result, "[REDACTED]");
}

// =============================================================================
// Patient ID Processing
// =============================================================================

#[test]
fn patient_id_hash_is_deterministic_across_calls() {
    let a = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
    let b = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
    assert_eq!(a, b, "same input must produce same hash");
}

#[test]
fn different_patient_ids_produce_different_hashes() {
    let a = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
    let b = scrub_patient_id("P456", &ScrubStrategy::Hash, TEST_SALT);
    assert_ne!(a, b, "different inputs must produce different hashes");
}

#[test]
fn different_salts_produce_different_hashes() {
    let a = scrub_patient_id("P123", &ScrubStrategy::Hash, "salt-aaa-at-least-32-characters-long");
    let b = scrub_patient_id("P123", &ScrubStrategy::Hash, "salt-bbb-at-least-32-characters-long");
    assert_ne!(a, b, "different salts must produce different hashes for same input");
}

#[test]
fn patient_id_for_storage_respects_internal_policy() {
    let strict = strict_pii_config();
    assert!(
        process_patient_id_for_storage("P123", &strict).starts_with("sha256:"),
        "strict mode scrubs for storage"
    );

    let permissive = permissive_internal_pii_config();
    assert_eq!(
        process_patient_id_for_storage("P123", &permissive),
        "P123",
        "permissive mode preserves for storage"
    );
}

// =============================================================================
// PHI Fields — Provider-Specific
// =============================================================================

#[test]
fn provider_a_phi_fields_not_in_pipeline_output() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let result = NormalizationPipeline::run(&ProviderA, input).unwrap();

    let json = serde_json::to_string(&result.assessments[0]).unwrap();
    assert!(!json.contains("Jane Doe"), "patient name must not appear in output");
    assert!(!json.contains("1990-05-15"), "DOB must not appear in output");
    assert!(!json.contains("\"name\""), "name field must not leak to output");
    assert!(!json.contains("\"dob\""), "dob field must not leak to output");
}

#[test]
fn provider_b_phi_fields_not_in_pipeline_output() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let result = NormalizationPipeline::run(&ProviderB, input).unwrap();

    let json = serde_json::to_string(&result.assessments[0]).unwrap();
    assert!(!json.contains("Jane Doe"), "patient name must not appear in output");
    assert!(!json.contains("patient_name"), "patient_name field must not leak to output");
}

// =============================================================================
// Audit Trail — PII Scrubbing is Recorded
// =============================================================================

#[test]
fn pipeline_audit_records_pii_scrub_step() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderA, input, Some(&config)).unwrap();

    let audit_payload = result.audit.to_payload("provider_a", "nested_json", 3);
    let transformations = audit_payload["transformations"].as_array().unwrap();

    let pii_step = transformations.iter().find(|t| t["step"] == "pii_scrub");
    assert!(pii_step.is_some(), "audit trail must record pii_scrub step");

    let step = pii_step.unwrap();
    assert_eq!(step["patient_id_scrubbed"], true);
}

#[test]
fn pipeline_audit_records_plaintext_when_internal_allows() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let config = permissive_internal_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderA, input, Some(&config)).unwrap();

    let audit_payload = result.audit.to_payload("provider_a", "nested_json", 3);
    let transformations = audit_payload["transformations"].as_array().unwrap();

    let pii_step = transformations.iter().find(|t| t["step"] == "pii_scrub");
    assert!(pii_step.is_some(), "audit trail must record pii_scrub step even when preserving");

    let step = pii_step.unwrap();
    assert_eq!(step["patient_id_scrubbed"], false, "should record that scrubbing was NOT applied");
    assert_eq!(step["strategy"], "plaintext_internal");
}

// =============================================================================
// Error Payload — No Raw Input Exposure
// =============================================================================

#[test]
fn error_payload_does_not_contain_raw_input_preview() {
    let raw = b"{\"patient\": {\"id\": \"P123\", \"name\": \"Jane Doe\"}}";
    let config = strict_pii_config();
    let scrubbed = pii::process_raw_input(raw, &config);

    assert!(!scrubbed.contains("Jane Doe"), "scrubbed raw input must not contain PHI");
    assert!(!scrubbed.contains("P123"), "scrubbed raw input must not contain PII");
    assert!(scrubbed.starts_with("sha256:"), "should be a hash");
}

// =============================================================================
// HMAC-SHA256 Salted Hashing Security
// =============================================================================

#[test]
fn hmac_hash_differs_from_plain_sha256() {
    use sha2::Digest;
    let plain_hash = sha2::Sha256::digest(b"P123");
    let plain = format!("sha256:{plain_hash:x}");
    let hmac_result = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);

    assert_ne!(plain, hmac_result, "HMAC hash must differ from plain SHA-256 to prevent rainbow tables");
    assert!(hmac_result.starts_with("sha256:"), "still uses sha256: prefix for format consistency");
}

#[test]
fn hmac_hash_is_consistent_within_environment() {
    let a = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
    let b = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
    assert_eq!(a, b, "same salt + same input = same hash (deterministic)");
}

#[test]
fn hmac_hash_prevents_cross_environment_correlation() {
    let staging = scrub_patient_id("P123", &ScrubStrategy::Hash, "staging-salt-that-is-at-least-32-chars");
    let production = scrub_patient_id("P123", &ScrubStrategy::Hash, "production-salt-is-different-at-32-chars");
    assert_ne!(staging, production, "different environments must produce different hashes");
}

// =============================================================================
// End-to-End Data Flow Verification
// =============================================================================

#[test]
fn full_pipeline_no_pii_leak_provider_a() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderA, input, Some(&config)).unwrap();

    // Serialize entire result to JSON and verify no PII leaks
    let full_json = serde_json::to_string(&result.assessments).unwrap();
    assert!(!full_json.contains("P123"), "patient_id P123 must not appear in output");
    assert!(!full_json.contains("Jane Doe"), "patient name must not appear in output");
    assert!(!full_json.contains("1990-05-15"), "DOB must not appear in output");

    // Verify the audit payload also doesn't leak PII
    let audit = result.audit.to_payload("provider_a", "nested_json", 3);
    let audit_json = serde_json::to_string(&audit).unwrap();
    assert!(!audit_json.contains("P123"), "audit must not contain patient_id");
    assert!(!audit_json.contains("Jane Doe"), "audit must not contain patient name");
}

#[test]
fn full_pipeline_no_pii_leak_provider_b() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderB, input, Some(&config)).unwrap();

    let full_json = serde_json::to_string(&result.assessments).unwrap();
    assert!(!full_json.contains("P123"));
    assert!(!full_json.contains("Jane Doe"));
}

#[test]
fn full_pipeline_no_pii_leak_provider_c() {
    let input = include_bytes!("../fixtures/provider_c.csv");
    let config = strict_pii_config();
    let result = NormalizationPipeline::run_with_pii(&ProviderC, input, Some(&config)).unwrap();

    let full_json = serde_json::to_string(&result.assessments).unwrap();
    assert!(!full_json.contains("P123"));
}
