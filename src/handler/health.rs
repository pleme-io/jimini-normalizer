use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::db;
use crate::state::AppState;

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    version: &'static str,
    database: &'static str,
}

/// Full health check — checks DB connectivity.
pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let db_ok = db::health_check(&state.db).await;
    Json(HealthResponse {
        status: if db_ok { "ok" } else { "degraded" },
        version: env!("CARGO_PKG_VERSION"),
        database: if db_ok { "ok" } else { "unreachable" },
    })
}

/// Liveness probe — the process is alive and not deadlocked.
pub async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — the service can handle requests (DB reachable).
pub async fn readyz(State(state): State<Arc<AppState>>) -> StatusCode {
    if db::health_check(&state.db).await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}
