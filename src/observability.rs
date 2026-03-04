use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use prometheus::TextEncoder;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::{debug, info, warn};

use crate::entity::{failed_record, outbox, schema_violation};

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

        gauge "jimini_failed_records_pending" => failed_records_pending: "Unreplayed failed records";

        gauge "jimini_schema_violations_recent" => schema_violations_recent: "Schema violations in last hour";

        gauge "jimini_outbox_pending" => outbox_pending: "Pending outbox messages";

        gauge "jimini_outbox_failed" => outbox_failed: "Failed outbox messages";

        // ─── NATS ──────────────────────────────────────────────────────────
        counter_vec "jimini_nats_publish_total" => nats_publish_total: "NATS publish attempts"
            labels: ["stream", "status"];

        counter "jimini_nats_dedup_total" => nats_dedup_total: "NATS outbox dedup hits";

        // ─── PostgresSink ──────────────────────────────────────────────────
        histogram "jimini_postgres_sink_duration_seconds" => postgres_sink_duration: "PostgresSink persist latency"
            labels: []
            buckets: [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0];

        counter_vec "jimini_postgres_sink_errors_total" => postgres_sink_errors: "PostgresSink errors"
            labels: ["error_type"];

        counter "jimini_postgres_sink_retries_total" => postgres_sink_retries: "PostgresSink transient retry attempts";

        // ─── Worker ────────────────────────────────────────────────────────
        counter_vec "jimini_worker_messages_total" => worker_messages_total: "Worker messages processed"
            labels: ["stream", "status"];

        // ─── Outbox Worker ─────────────────────────────────────────────────
        counter_vec "jimini_outbox_processed_total" => outbox_processed_total: "Outbox messages processed"
            labels: ["status"];

        counter "jimini_outbox_restarts_total" => outbox_restarts_total: "Outbox worker restarts";

        // ─── External Sinks ───────────────────────────────────────────────
        counter_vec "jimini_sink_delivery_total" => sink_delivery_total: "External sink delivery attempts"
            labels: ["sink", "status"];

        histogram "jimini_sink_delivery_duration_seconds" => sink_delivery_duration: "External sink delivery latency"
            labels: ["sink"]
            buckets: [0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

        // ─── Sweeper ──────────────────────────────────────────────────────
        counter_vec "jimini_sweeper_runs_total" => sweeper_runs_total: "Sweeper cycle runs"
            labels: ["status"];

        counter "jimini_sweeper_entries_redriven_total" => sweeper_entries_redriven: "Entries re-driven by sweeper";
        counter "jimini_sweeper_entries_failed_total" => sweeper_entries_failed: "Entries failed after max retries";

        // ─── Batch ────────────────────────────────────────────────────────
        counter "jimini_dead_letters_total" => dead_letters_total: "Dead letters from batch emit";

        histogram "jimini_batch_size" => batch_size: "Batch normalize request sizes"
            labels: []
            buckets: [1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0];

        // ─── Replay ───────────────────────────────────────────────────────
        counter_vec "jimini_replay_total" => replay_total: "Replay operations"
            labels: ["status"];
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

/// Standalone metrics HTTP server on the dedicated metrics port.
///
/// Exposes `/metrics` and `/health` (always-ok liveness) on a separate port
/// so Prometheus can scrape both API and Worker pods independently.
pub async fn run_metrics_server(addr: SocketAddr) {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(|| async { "ok" }));

    info!(%addr, "starting metrics server");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
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

    http_requests_total()
        .with_label_values(&[&method, &path, &status])
        .inc();
    http_request_duration()
        .with_label_values(&[&method, &path])
        .observe(duration);

    response
}

/// Background task that refreshes audit gauge metrics every 60 seconds.
/// Runs in API mode only — provides real-time Prometheus scrape data.
pub async fn audit_gauge_worker(db: DatabaseConnection) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;
        refresh_audit_gauges(&db).await;
    }
}

async fn refresh_audit_gauges(db: &DatabaseConnection) {
    // Unreplayed failed records
    match failed_record::Entity::find()
        .filter(failed_record::Column::Replayed.eq(false))
        .all(db)
        .await
    {
        Ok(records) => {
            failed_records_pending().set(records.len() as i64);
            debug!(count = records.len(), "audit gauges: failed_records_pending");
        }
        Err(e) => warn!(error = %e, "audit gauges: failed to query failed_records"),
    }

    // Schema violations in last hour
    let one_hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    match schema_violation::Entity::find()
        .filter(schema_violation::Column::CreatedAt.gte(one_hour_ago))
        .all(db)
        .await
    {
        Ok(violations) => {
            schema_violations_recent().set(violations.len() as i64);
            debug!(
                count = violations.len(),
                "audit gauges: schema_violations_recent"
            );
        }
        Err(e) => warn!(error = %e, "audit gauges: failed to query schema_violations"),
    }

    // Outbox pending
    match outbox::Entity::find()
        .filter(outbox::Column::Status.eq("pending"))
        .all(db)
        .await
    {
        Ok(records) => {
            outbox_pending().set(records.len() as i64);
        }
        Err(e) => warn!(error = %e, "audit gauges: failed to query outbox pending"),
    }

    // Outbox failed
    match outbox::Entity::find()
        .filter(outbox::Column::Status.eq("failed"))
        .all(db)
        .await
    {
        Ok(records) => {
            outbox_failed().set(records.len() as i64);
        }
        Err(e) => warn!(error = %e, "audit gauges: failed to query outbox failed"),
    }
}
