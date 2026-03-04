mod common;

use axum_test::TestServer;
use jimini_normalizer::router::build_router;

async fn test_server() -> (TestServer, common::TestInfra) {
    let infra = common::TestInfra::start().await;
    let state = infra.app_state_with_pii();

    // Ensure NATS streams exist (required for normalize handlers)
    jimini_normalizer::nats::ensure_streams(&infra.js, &state.config())
        .await
        .expect("failed to ensure NATS streams");

    let app = build_router(state);
    let server = TestServer::new(app).unwrap();
    (server, infra)
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let (server, _infra) = test_server().await;
    let response = server.get("/health").await;
    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["database"], "ok");
}

#[tokio::test]
async fn normalize_provider_a_returns_unified_assessment() {
    let (server, _infra) = test_server().await;
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
    let patient_id = a["patientId"].as_str().unwrap();
    // patient_id is scrubbed in API response (hashed) — must NOT be plaintext
    assert!(patient_id.starts_with("sha256:"), "patientId must be hashed in API response, got: {patient_id}");
    assert!(!patient_id.contains("P123"), "plaintext patient_id must NOT appear in API response");
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
    assert!(!raw.contains("P123"), "plaintext patient_id must NOT appear in serialized output");
}

#[tokio::test]
async fn normalize_provider_b_extracts_score_prefix_keys() {
    let (server, _infra) = test_server().await;
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
    let patient_id = a["patientId"].as_str().unwrap();
    assert!(patient_id.starts_with("sha256:"), "patientId must be hashed");
    assert_eq!(a["assessmentType"], "cognitive");
    assert_eq!(a["metadata"]["sourceFormat"], "flat_kv");

    let scores = a["scores"].as_array().unwrap();
    assert_eq!(scores.len(), 2); // score_memory and score_processing
}

#[tokio::test]
async fn normalize_provider_c_csv_input() {
    let (server, _infra) = test_server().await;
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
    let patient_id = a["patientId"].as_str().unwrap();
    assert!(patient_id.starts_with("sha256:"), "patientId must be hashed");
    assert_eq!(a["assessmentType"], "behavioral");
    assert_eq!(a["scores"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn normalize_unknown_provider_returns_400() {
    let (server, _infra) = test_server().await;

    let response = server
        .post("/normalize")
        .add_query_param("provider", "unknown")
        .content_type("application/json")
        .text("{}")
        .await;

    response.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

// NOTE: assessments_round_trip test removed — with NATS architecture,
// DB persistence happens asynchronously via the worker, not inline in
// the API handler. Use worker integration tests for DB round-trip verification.

#[tokio::test]
async fn batch_normalize_with_partial_failure() {
    let (server, _infra) = test_server().await;

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
    let (server, _infra) = test_server().await;

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

// NOTE: failed_records and audit_logs tests removed — with NATS architecture,
// error persistence happens asynchronously via the error worker.
// The normalize response is still correct; DB writes are async.

#[tokio::test]
async fn provider_a_scores_are_scaled_0_10_to_0_100() {
    let (server, _infra) = test_server().await;
    let input = include_str!("../fixtures/provider_a.json");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_a")
        .content_type("application/json")
        .text(input)
        .await;
    response.assert_status_ok();

    let body = response.json::<serde_json::Value>();
    let scores = body[0]["scores"].as_array().unwrap();

    // Find each dimension and verify scaling (0-10 → 0-100)
    let find_score = |dim: &str| -> f64 {
        scores
            .iter()
            .find(|s| s["dimension"] == dim)
            .unwrap()["value"]
            .as_f64()
            .unwrap()
    };

    assert_eq!(find_score("anxiety"), 70.0);    // 7 → 70
    assert_eq!(find_score("social"), 40.0);     // 4 → 40
    assert_eq!(find_score("attention"), 60.0);  // 6 → 60

    // All scores must report 0-100 scale
    for score in scores {
        assert_eq!(score["scale"], "0-100");
    }
}

#[tokio::test]
async fn provider_c_scores_are_scaled_0_10_to_0_100() {
    let (server, _infra) = test_server().await;
    let input = include_str!("../fixtures/provider_c.csv");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_c")
        .content_type("text/csv")
        .text(input)
        .await;
    response.assert_status_ok();

    let body = response.json::<serde_json::Value>();
    let scores = body[0]["scores"].as_array().unwrap();

    let find_score = |dim: &str| -> f64 {
        scores
            .iter()
            .find(|s| s["dimension"] == dim)
            .unwrap()["value"]
            .as_f64()
            .unwrap()
    };

    assert_eq!(find_score("attention_span"), 60.0);      // 6 → 60
    assert_eq!(find_score("social_engagement"), 40.0);   // 4 → 40
}

#[tokio::test]
async fn provider_b_phi_fields_are_excluded() {
    let (server, _infra) = test_server().await;
    let input = include_str!("../fixtures/provider_b.json");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_b")
        .content_type("application/json")
        .text(input)
        .await;
    response.assert_status_ok();

    let raw = response.text();
    assert!(!raw.contains("Jane Doe"), "patient name must not appear in output");
    assert!(!raw.contains("patient_name"), "patient_name field must not appear in output");
}

#[tokio::test]
async fn list_assessments_returns_array() {
    let (server, _infra) = test_server().await;

    let response = server.get("/assessments").await;
    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    assert!(body.is_array());
}

#[tokio::test]
async fn get_nonexistent_assessment_returns_404() {
    let (server, _infra) = test_server().await;

    let response = server
        .get("/assessments/00000000-0000-0000-0000-000000000000")
        .await;
    response.assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn schema_violations_endpoint_returns_ok() {
    let (server, _infra) = test_server().await;

    let response = server.get("/audit/violations").await;
    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    assert!(body.is_array());
}

#[tokio::test]
async fn malformed_input_returns_client_error() {
    let (server, _infra) = test_server().await;

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_a")
        .content_type("application/json")
        .text("not json at all")
        .await;

    let status = response.status_code().as_u16();
    assert!(
        status == 400 || status == 422,
        "malformed input should return 400 or 422, got {status}"
    );
}

#[tokio::test]
async fn output_uses_camel_case_field_names() {
    let (server, _infra) = test_server().await;
    let input = include_str!("../fixtures/provider_a.json");

    let response = server
        .post("/normalize")
        .add_query_param("provider", "provider_a")
        .content_type("application/json")
        .text(input)
        .await;
    response.assert_status_ok();

    let raw = response.text();
    assert!(raw.contains("patientId"), "output must use camelCase patientId");
    assert!(raw.contains("assessmentType"), "output must use camelCase assessmentType");
    assert!(raw.contains("sourceProvider"), "output must use camelCase sourceProvider");
    assert!(raw.contains("assessmentDate"), "output must use camelCase assessmentDate");
    // Must NOT contain snake_case field names (note: "patient_id" may appear as
    // part of "sha256:..." hash value, so we check for the JSON key pattern)
    assert!(!raw.contains("\"patient_id\""), "output must not use snake_case patient_id key");
    assert!(!raw.contains("\"assessment_type\""), "output must not use snake_case assessment_type key");
}
