use jimini_normalizer::audit_checker;
use jimini_normalizer::config;
use jimini_normalizer::db;
use jimini_normalizer::dynamic_config;
use jimini_normalizer::nats;
use jimini_normalizer::observability;
use jimini_normalizer::outbox;
use jimini_normalizer::retention;
use jimini_normalizer::router::build_router;
use jimini_normalizer::run_mode::RunMode;
use jimini_normalizer::state::AppState;
use jimini_normalizer::worker;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    let cfg = config::load();
    let run_mode = RunMode::from_env();

    // Initialize observability: auto-detects OTEL if OTEL_EXPORTER_OTLP_ENDPOINT is set,
    // otherwise falls back to structured JSON logging
    pleme_observability::init_observability("jimini-normalizer");

    info!(%run_mode, "jimini-normalizer starting");

    // Connect to PostgreSQL
    let db_conn = db::connect(&cfg.database.url)
        .await
        .expect("failed to connect to database");

    match run_mode {
        RunMode::Migrate => {
            db::run_migrations(&db_conn)
                .await
                .expect("failed to run migrations");
            info!("migrations complete, exiting");
        }
        RunMode::AuditChecker => {
            // Run migrations first (idempotent) to ensure schema is current
            db::run_migrations(&db_conn)
                .await
                .expect("failed to run migrations");

            let exit_code = audit_checker::run(&db_conn).await;
            std::process::exit(exit_code);
        }
        RunMode::Retention => {
            db::run_migrations(&db_conn)
                .await
                .expect("failed to run migrations");

            match retention::purge_expired(&db_conn, cfg.retention.days).await {
                Ok(report) => {
                    info!(?report, "retention purge completed successfully");
                }
                Err(e) => {
                    error!(error = %e, "retention purge failed");
                    std::process::exit(1);
                }
            }
        }
        RunMode::Worker => {
            db::run_migrations(&db_conn)
                .await
                .expect("failed to run migrations");

            // Connect to NATS
            let nats_client = nats::connect(&cfg.nats.url)
                .await
                .expect("failed to connect to NATS");
            let js = nats::jetstream_context(&nats_client);

            // Ensure streams exist
            nats::ensure_streams(&js, cfg)
                .await
                .expect("failed to ensure NATS streams");

            // Spawn the audit gauge refresh background task
            let gauge_db = db_conn.clone();
            tokio::spawn(async move {
                observability::audit_gauge_worker(gauge_db).await;
            });

            // Spawn standalone metrics server for Prometheus scraping
            let metrics_addr = cfg.metrics_socket_addr();
            tokio::spawn(async move {
                observability::run_metrics_server(metrics_addr).await;
            });

            // Create shared state for the worker
            let state = AppState::new(cfg.clone(), db_conn, js);

            // Spawn dynamic config refresh task
            dynamic_config::spawn_refresh_task(state.clone(), cfg.clone());

            // Spawn outbox worker (delivers assessments to configured sinks)
            let outbox_state = state.clone();
            let outbox_handle = tokio::spawn(async move {
                outbox::run_outbox_worker(outbox_state).await;
            });

            // Spawn reconciliation sweeper (re-drives stale pending outbox entries)
            let sweeper_state = state.clone();
            let sweeper_handle = tokio::spawn(async move {
                outbox::run_reconciliation_sweeper(sweeper_state).await;
            });

            // Monitor all background tasks — if any panic, log and exit
            let worker_state = state.clone();
            let worker_handle = tokio::spawn(async move {
                worker::run(worker_state).await;
            });

            info!("starting NATS worker");
            tokio::select! {
                result = worker_handle => {
                    match result {
                        Ok(()) => error!("NATS worker exited unexpectedly"),
                        Err(e) => error!(error = %e, "NATS worker panicked"),
                    }
                }
                result = outbox_handle => {
                    match result {
                        Ok(()) => error!("outbox worker exited unexpectedly"),
                        Err(e) => error!(error = %e, "outbox worker panicked"),
                    }
                }
                result = sweeper_handle => {
                    match result {
                        Ok(()) => error!("reconciliation sweeper exited unexpectedly"),
                        Err(e) => error!(error = %e, "reconciliation sweeper panicked"),
                    }
                }
            }
        }
        RunMode::Api => {
            // Run migrations on startup
            db::run_migrations(&db_conn)
                .await
                .expect("failed to run migrations");

            // Connect to NATS
            let nats_client = nats::connect(&cfg.nats.url)
                .await
                .expect("failed to connect to NATS");
            let js = nats::jetstream_context(&nats_client);

            // Ensure streams exist
            nats::ensure_streams(&js, cfg)
                .await
                .expect("failed to ensure NATS streams");

            // Spawn the audit gauge refresh background task
            let gauge_db = db_conn.clone();
            tokio::spawn(async move {
                observability::audit_gauge_worker(gauge_db).await;
            });

            // Spawn standalone metrics server on dedicated port for Prometheus
            let metrics_addr = cfg.metrics_socket_addr();
            tokio::spawn(async move {
                observability::run_metrics_server(metrics_addr).await;
            });

            let addr = cfg.socket_addr();
            let state = AppState::new(cfg.clone(), db_conn, js);

            // Spawn dynamic config refresh task
            dynamic_config::spawn_refresh_task(state.clone(), cfg.clone());

            // Spawn outbox worker (delivers assessments to configured sinks)
            let outbox_state = state.clone();
            let outbox_handle = tokio::spawn(async move {
                outbox::run_outbox_worker(outbox_state).await;
            });

            // Spawn reconciliation sweeper (re-drives stale pending outbox entries)
            let sweeper_state = state.clone();
            let sweeper_handle = tokio::spawn(async move {
                outbox::run_reconciliation_sweeper(sweeper_state).await;
            });

            let app = build_router(state);

            info!(%addr, "starting HTTP server");

            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            let server_handle = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });

            // Monitor all background tasks — if any panic, log and exit
            tokio::select! {
                result = server_handle => {
                    match result {
                        Ok(()) => error!("HTTP server exited unexpectedly"),
                        Err(e) => error!(error = %e, "HTTP server panicked"),
                    }
                }
                result = outbox_handle => {
                    match result {
                        Ok(()) => error!("outbox worker exited unexpectedly"),
                        Err(e) => error!(error = %e, "outbox worker panicked"),
                    }
                }
                result = sweeper_handle => {
                    match result {
                        Ok(()) => error!("reconciliation sweeper exited unexpectedly"),
                        Err(e) => error!(error = %e, "reconciliation sweeper panicked"),
                    }
                }
            }
        }
    }
}
