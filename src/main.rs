use jimini_normalizer::config::AppConfig;
use jimini_normalizer::router::build_router;
use jimini_normalizer::state::AppState;
use tracing::info;

#[tokio::main]
async fn main() {
    let config = AppConfig::from_env();

    // Initialize observability: auto-detects OTEL if OTEL_EXPORTER_OTLP_ENDPOINT is set,
    // otherwise falls back to structured JSON logging
    pleme_observability::init_observability("jimini-normalizer");

    let addr = config.socket_addr();
    let state = AppState::new(config);
    let app = build_router(state);

    info!(%addr, "starting jimini-normalizer");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
