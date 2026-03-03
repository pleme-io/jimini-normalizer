use jimini_normalizer::pipeline::NormalizationPipeline;
use jimini_normalizer::provider::provider_a::ProviderA;
use jimini_normalizer::provider::provider_b::ProviderB;

#[test]
fn pipeline_rejects_invalid_input() {
    let result = NormalizationPipeline::run(&ProviderA, b"not json");
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("parse error"));
}

#[test]
fn pipeline_validates_output_scores_in_range() {
    // Score value of 15 on 0-10 scale → 150 after scaling → should fail output validation
    let input = serde_json::json!({
        "patient": { "id": "P-BAD", "name": "Test" },
        "assessment": {
            "type": "test",
            "scores": { "test_dim": 15.0 }
        }
    });
    let raw = serde_json::to_vec(&input).unwrap();
    let result = NormalizationPipeline::run(&ProviderA, &raw);
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("validation"));
}

#[test]
fn pipeline_succeeds_for_valid_provider_b_input() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let result = NormalizationPipeline::run(&ProviderB, input);
    assert!(result.is_ok());

    let assessments = result.unwrap();
    assert_eq!(assessments.len(), 1);
    for score in &assessments[0].scores {
        assert!(score.value >= 0.0 && score.value <= 100.0);
    }
}

#[test]
fn pipeline_output_uses_camel_case_json() {
    let input = include_bytes!("../fixtures/provider_b.json");
    let assessments = NormalizationPipeline::run(&ProviderB, input).unwrap();
    let json = serde_json::to_string(&assessments[0]).unwrap();

    // Verify camelCase field names in serialized output
    assert!(json.contains("\"patientId\""));
    assert!(json.contains("\"assessmentDate\""));
    assert!(json.contains("\"assessmentType\""));
    assert!(json.contains("\"sourceProvider\""));
    assert!(json.contains("\"sourceFormat\""));
    assert!(json.contains("\"ingestedAt\""));

    // Verify NO snake_case leaks
    assert!(!json.contains("\"patient_id\""));
    assert!(!json.contains("\"assessment_date\""));
    assert!(!json.contains("\"source_provider\""));
}

#[test]
fn pipeline_does_not_propagate_phi_fields() {
    let input = include_bytes!("../fixtures/provider_a.json");
    let assessments = NormalizationPipeline::run(&ProviderA, input).unwrap();
    let json = serde_json::to_string(&assessments[0]).unwrap();

    // Patient name and DOB from input must NOT appear in normalized output
    assert!(!json.contains("Jane Doe"));
    assert!(!json.contains("1990-05-15"));
    assert!(!json.contains("\"name\""));
    assert!(!json.contains("\"dob\""));
}
