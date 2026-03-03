use axum_test::TestServer;
use jimini_normalizer::config::AppConfig;
use jimini_normalizer::model::unified::UnifiedAssessment;
use jimini_normalizer::router::build_router;
use jimini_normalizer::state::AppState;

fn test_server() -> TestServer {
    let config = AppConfig::from_env();
    let state = AppState::new(config);
    let app = build_router(state);
    TestServer::new(app).unwrap()
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let server = test_server();
    let response = server.get("/health").await;
    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn normalize_provider_a_returns_unified_assessment() {
    let server = test_server();
    let input = include_str!("../fixtures/provider_a.json");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_a")
        .content_type("application/json")
        .text(input)
        .await;

    response.assert_status_ok();
    let assessments = response.json::<Vec<UnifiedAssessment>>();
    assert_eq!(assessments.len(), 1);
    assert_eq!(assessments[0].patient_id, "PAT-001");
    assert_eq!(assessments[0].metadata.source_provider, "provider_a");
}

#[tokio::test]
async fn normalize_unknown_provider_returns_400() {
    let server = test_server();

    let response = server
        .post("/normalize")
        .add_query_param("provider", "unknown")
        .content_type("application/json")
        .text("{}")
        .await;

    response.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn assessments_round_trip() {
    let server = test_server();
    let input = include_str!("../fixtures/provider_b.json");

    // Normalize and store
    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_b")
        .content_type("application/json")
        .text(input)
        .await;
    response.assert_status_ok();
    let assessments = response.json::<Vec<UnifiedAssessment>>();
    let id = assessments[0].id;

    // Retrieve by ID
    let response = server.get(&format!("/assessments/{id}")).await;
    response.assert_status_ok();
    let retrieved = response.json::<UnifiedAssessment>();
    assert_eq!(retrieved.id, id);
    assert_eq!(retrieved.patient_id, "PAT-002");
}

#[tokio::test]
async fn batch_normalize_with_partial_failure() {
    let server = test_server();

    let batch = serde_json::json!([
        {
            "provider": "provider_b",
            "data": {
                "patient_id": "PAT-010",
                "assessment_date": "2024-01-01T00:00:00Z",
                "assessment_type": "GAD-7",
                "scores": { "anxiety": 50.0 }
            }
        },
        {
            "provider": "unknown_provider",
            "data": {}
        }
    ]);

    let response = server
        .post("/normalize/batch")
        .content_type("application/json")
        .json(&batch)
        .await;

    // 207 Multi-Status for partial failure
    response.assert_status(axum::http::StatusCode::MULTI_STATUS);
    let body = response.json::<serde_json::Value>();
    assert_eq!(body["successes"].as_array().unwrap().len(), 1);
    assert_eq!(body["errors"].as_array().unwrap().len(), 1);
}
