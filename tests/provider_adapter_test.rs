use jimini_normalizer::provider::NormalizationProvider;
use jimini_normalizer::provider::provider_a::ProviderA;
use jimini_normalizer::provider::provider_b::ProviderB;
use jimini_normalizer::provider::provider_c::ProviderC;

#[test]
fn provider_a_normalizes_nested_json_with_score_scaling() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let provider = ProviderA;

    assert!(provider.validate_input(input).is_ok());

    let assessments = provider.normalize(input).unwrap();
    assert_eq!(assessments.len(), 1);

    let a = &assessments[0];
    assert_eq!(a.patient_id, "P123");
    assert_eq!(a.assessment_type, "behavioral_screening");
    assert_eq!(a.scores.len(), 3);
    assert_eq!(a.metadata.source_provider, "provider_a");
    assert_eq!(a.metadata.source_format, "nested_json");

    // Verify 0-10 → 0-100 scaling
    let anxiety = a.scores.iter().find(|s| s.dimension == "anxiety").unwrap();
    assert!((anxiety.value - 70.0).abs() < f64::EPSILON);
    assert_eq!(anxiety.scale, "0-100");

    let social = a.scores.iter().find(|s| s.dimension == "social").unwrap();
    assert!((social.value - 40.0).abs() < f64::EPSILON);
}

#[test]
fn provider_b_extracts_score_prefixed_keys() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let provider = ProviderB;

    let assessments = provider.normalize(input).unwrap();
    assert_eq!(assessments.len(), 1);

    let a = &assessments[0];
    assert_eq!(a.patient_id, "P123");
    assert_eq!(a.assessment_type, "cognitive");
    assert_eq!(a.metadata.source_format, "flat_kv");

    // Scores extracted from score_* keys, already 0-100
    let memory = a.scores.iter().find(|s| s.dimension == "memory").unwrap();
    assert!((memory.value - 85.0).abs() < f64::EPSILON);

    let processing = a.scores.iter().find(|s| s.dimension == "processing").unwrap();
    assert!((processing.value - 72.0).abs() < f64::EPSILON);

    // patient_name and notes are NOT in the output (PHI not propagated)
    let json = serde_json::to_string(a).unwrap();
    assert!(!json.contains("Jane Doe"));
}

#[test]
fn provider_c_normalizes_csv_with_row_grouping() {
    let input = include_bytes!("../fixtures/provider_c.csv");
    let provider = ProviderC;

    let assessments = provider.normalize(input).unwrap();
    assert_eq!(assessments.len(), 1); // Both rows share patient/date/category

    let a = &assessments[0];
    assert_eq!(a.patient_id, "P123");
    assert_eq!(a.assessment_type, "behavioral");
    assert_eq!(a.scores.len(), 2);

    // Verify 0-10 → 0-100 scaling
    let attention = a.scores.iter().find(|s| s.dimension == "attention_span").unwrap();
    assert!((attention.value - 60.0).abs() < f64::EPSILON);

    let social = a.scores.iter().find(|s| s.dimension == "social_engagement").unwrap();
    assert!((social.value - 40.0).abs() < f64::EPSILON);
}

#[test]
fn provider_a_rejects_malformed_input() {
    let provider = ProviderA;
    let result = provider.validate_input(b"not json");
    assert!(result.is_err());
}

#[test]
fn provider_b_rejects_missing_score_keys() {
    let input = br#"{"patient_id":"P1","assessment_type":"test","notes":"x"}"#;
    let provider = ProviderB;
    let result = provider.validate_input(input);
    assert!(result.is_err());
}
