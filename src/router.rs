use std::sync::Arc;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::handler::{admin_config, assessments, audit, failed_records, health, normalize, retention};
use crate::observability;
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/normalize", post(normalize::normalize))
        .route("/normalize/batch", post(normalize::normalize_batch))
        .route("/assessments", get(assessments::list_assessments))
        .route("/assessments/{id}", get(assessments::get_assessment))
        // Audit trail
        .route("/audit/logs", get(audit::list_audit_logs))
        .route("/audit/violations", get(audit::list_schema_violations))
        // Failed records (exploration + replay)
        .route("/failed", get(failed_records::list_failed_records))
        .route("/failed/{id}", get(failed_records::get_failed_record))
        .route(
            "/failed/{id}/replay",
            post(failed_records::replay_failed_record),
        )
        // Retention (K8s CronJob calls this via curl)
        .route("/admin/purge", post(retention::purge))
        // Dynamic config admin API
        .route("/admin/config", get(admin_config::list_config))
        .route(
            "/admin/config/{key}",
            get(admin_config::get_config)
                .put(admin_config::set_config)
                .delete(admin_config::delete_config),
        )
        .route("/metrics", get(observability::metrics_handler))
        .layer(middleware::from_fn(observability::track_metrics))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
