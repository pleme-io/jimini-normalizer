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
    let body = response.json::<serde_json::Value>();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);

    let a = &arr[0];
    // Verify camelCase output matching PDF unified schema
    assert_eq!(a["patientId"], "P123");
    assert_eq!(a["assessmentType"], "behavioral_screening");
    assert_eq!(a["metadata"]["sourceProvider"], "provider_a");
    assert_eq!(a["metadata"]["sourceFormat"], "nested_json");

    // Scores are scaled 0-10 → 0-100
    let scores = a["scores"].as_array().unwrap();
    assert_eq!(scores.len(), 3);

    // PHI is NOT in the output
    let raw = serde_json::to_string(&a).unwrap();
    assert!(!raw.contains("Jane Doe"));
    assert!(!raw.contains("1990-05-15"));
}

#[tokio::test]
async fn normalize_provider_b_extracts_score_prefix_keys() {
    let server = test_server();
    let input = include_str!("../fixtures/provider_b.json");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_b")
        .content_type("application/json")
        .text(input)
        .await;

    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    let a = &body.as_array().unwrap()[0];
    assert_eq!(a["patientId"], "P123");
    assert_eq!(a["assessmentType"], "cognitive");
    assert_eq!(a["metadata"]["sourceFormat"], "flat_kv");

    let scores = a["scores"].as_array().unwrap();
    assert_eq!(scores.len(), 2); // score_memory and score_processing
}

#[tokio::test]
async fn normalize_provider_c_csv_input() {
    let server = test_server();
    let input = include_str!("../fixtures/provider_c.csv");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_c")
        .content_type("text/csv")
        .text(input)
        .await;

    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);

    let a = &arr[0];
    assert_eq!(a["patientId"], "P123");
    assert_eq!(a["assessmentType"], "behavioral");
    assert_eq!(a["scores"].as_array().unwrap().len(), 2);
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
    let input = include_str!("../fixtures/provider_a.json");

    // Normalize and store
    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_a")
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
    assert_eq!(retrieved.patient_id, "P123");
}

#[tokio::test]
async fn batch_normalize_with_partial_failure() {
    let server = test_server();

    let batch = serde_json::json!([
        {
            "provider": "provider_b",
            "data": {
                "patient_id": "P456",
                "assessment_type": "cognitive",
                "score_memory": 90
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

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_format() {
    let server = test_server();

    // Make a request first so metrics have data
    let input = include_str!("../fixtures/provider_a.json");
    server
        .post("/normalize")
        .add_query_param("provider", "provider_a")
        .content_type("application/json")
        .text(input)
        .await;

    let response = server.get("/metrics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("jimini_http_requests_total"));
    assert!(body.contains("jimini_normalizations_total"));
}
