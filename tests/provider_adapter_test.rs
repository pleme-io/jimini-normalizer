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
    assert_eq!(a.patient_id, "PAT-001");
    assert_eq!(a.assessment_type, "PHQ-9");
    assert_eq!(a.scores.len(), 3);
    assert_eq!(a.metadata.source_provider, "provider_a");

    // Verify 0-10 → 0-100 scaling
    let mood = a.scores.iter().find(|s| s.dimension == "mood").unwrap();
    assert!((mood.value - 75.0).abs() < f64::EPSILON);
    assert_eq!(mood.scale, "0-100");

    let sleep = a.scores.iter().find(|s| s.dimension == "sleep").unwrap();
    assert!((sleep.value - 40.0).abs() < f64::EPSILON);
}

#[test]
fn provider_b_normalizes_flat_kv_without_scaling() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let provider = ProviderB;

    let assessments = provider.normalize(input).unwrap();
    assert_eq!(assessments.len(), 1);

    let a = &assessments[0];
    assert_eq!(a.patient_id, "PAT-002");
    assert_eq!(a.assessment_type, "GAD-7");
    assert_eq!(a.metadata.source_format, "json_flat");

    // Scores are already 0-100, no scaling
    let anxiety = a.scores.iter().find(|s| s.dimension == "anxiety").unwrap();
    assert!((anxiety.value - 65.0).abs() < f64::EPSILON);
}

#[test]
fn provider_c_normalizes_csv_with_row_grouping() {
    let input = include_bytes!("../fixtures/provider_c.csv");
    let provider = ProviderC;

    let assessments = provider.normalize(input).unwrap();
    assert_eq!(assessments.len(), 1); // All rows belong to same patient/date/type

    let a = &assessments[0];
    assert_eq!(a.patient_id, "PAT-003");
    assert_eq!(a.assessment_type, "BDI-II");
    assert_eq!(a.scores.len(), 3);

    // Verify 0-10 → 0-100 scaling
    let mood = a.scores.iter().find(|s| s.dimension == "mood").unwrap();
    assert!((mood.value - 80.0).abs() < f64::EPSILON);
}

#[test]
fn provider_a_rejects_malformed_input() {
    let provider = ProviderA;
    let result = provider.validate_input(b"not json");
    assert!(result.is_err());
}
