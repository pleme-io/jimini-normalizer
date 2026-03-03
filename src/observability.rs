use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::MatchedPath;
use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use prometheus::TextEncoder;

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(|| Metrics::new().expect("failed to initialize metrics"))
}

pleme_observability::define_metrics! {
    metrics_fn: crate::observability::metrics;

    struct Metrics {
        counter_vec "jimini_http_requests_total" => http_requests_total: "Total HTTP requests"
            labels: ["method", "path", "status"];

        histogram "jimini_http_request_duration_seconds" => http_request_duration: "HTTP request duration in seconds"
            labels: ["method", "path"]
            buckets: [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

        counter_vec "jimini_normalizations_total" => normalizations_total: "Total normalization attempts"
            labels: ["provider", "status"];

        counter "jimini_assessments_stored_total" => assessments_stored: "Total assessments stored";
    }
}

pub async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = metrics().registry().gather();
    let body = encoder.encode_to_string(&metric_families).unwrap_or_default();
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
}

pub async fn track_metrics(req: Request<Body>, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    http_requests_total().with_label_values(&[&method, &path, &status]).inc();
    http_request_duration().with_label_values(&[&method, &path]).observe(duration);

    response
}
