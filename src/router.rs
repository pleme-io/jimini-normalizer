use std::sync::Arc;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::handler::{assessments, health, normalize};
use crate::observability;
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/normalize", post(normalize::normalize))
        .route("/normalize/batch", post(normalize::normalize_batch))
        .route("/assessments", get(assessments::list_assessments))
        .route("/assessments/:id", get(assessments::get_assessment))
        .route("/metrics", get(observability::metrics_handler))
        .layer(middleware::from_fn(observability::track_metrics))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
