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
        "patient": { "id": "PAT-BAD" },
        "assessment": {
            "date": "2024-01-01T00:00:00Z",
            "type": "TEST",
            "scores": [
                { "category": "test", "value": 15.0 }
            ]
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
