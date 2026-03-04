//! Database bootstrap binary for docker-compose and upstream environments.
//!
//! Creates the target database (if it doesn't exist), runs all migrations,
//! and validates schema health. Designed to run as an init container or
//! docker-compose dependency before API/worker services start.
//!
//! Usage:
//!   DATABASE_URL=postgres://user:pass@host:5432/dbname db-bootstrap
//!
//! Exit codes:
//!   0 — success (database ready)
//!   1 — failure (connection, migration, or validation error)

use std::process::ExitCode;

use sea_orm::{ConnectOptions, ConnectionTrait, Database};
use tracing::{error, info};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let database_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            error!("DATABASE_URL environment variable is required");
            return ExitCode::FAILURE;
        }
    };

    info!("db-bootstrap starting");

    // Parse the URL to extract the target database name
    let (server_url, db_name) = match parse_database_url(&database_url) {
        Some(parsed) => parsed,
        None => {
            error!("failed to parse DATABASE_URL — expected postgres://user:pass@host:port/dbname");
            return ExitCode::FAILURE;
        }
    };

    // Step 1: Connect to the server (postgres database) and create target DB
    info!(database = %db_name, "ensuring database exists");
    match ensure_database_exists(&server_url, &db_name).await {
        Ok(()) => info!(database = %db_name, "database exists"),
        Err(e) => {
            error!(error = %e, "failed to ensure database exists");
            return ExitCode::FAILURE;
        }
    }

    // Step 2: Connect to the target database and run migrations
    info!("running migrations");
    match run_migrations(&database_url).await {
        Ok(()) => info!("migrations complete"),
        Err(e) => {
            error!(error = %e, "migration failed");
            return ExitCode::FAILURE;
        }
    }

    // Step 3: Validate schema health
    info!("validating schema health");
    match validate_schema(&database_url).await {
        Ok(()) => info!("schema validation passed"),
        Err(e) => {
            error!(error = %e, "schema validation failed");
            return ExitCode::FAILURE;
        }
    }

    info!("db-bootstrap complete — database is ready");
    ExitCode::SUCCESS
}

/// Parse a DATABASE_URL into (server_url_with_postgres_db, target_db_name).
fn parse_database_url(url: &str) -> Option<(String, String)> {
    // postgres://user:pass@host:port/dbname → extract dbname, replace with "postgres"
    let last_slash = url.rfind('/')?;
    let db_name = url[last_slash + 1..].to_string();
    if db_name.is_empty() {
        return None;
    }
    let server_url = format!("{}postgres", &url[..last_slash + 1]);
    Some((server_url, db_name))
}

async fn ensure_database_exists(server_url: &str, db_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut opts = ConnectOptions::new(server_url);
    opts.sqlx_logging(false);
    let db = Database::connect(opts).await?;

    // Check if database exists
    let result = db
        .query_one(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT 1 FROM pg_database WHERE datname = '{db_name}'"),
        ))
        .await?;

    if result.is_none() {
        info!(database = %db_name, "creating database");
        db.execute_unprepared(&format!("CREATE DATABASE \"{db_name}\""))
            .await?;
    }

    db.close().await?;
    Ok(())
}

async fn run_migrations(database_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use migration::{Migrator, MigratorTrait};

    let mut opts = ConnectOptions::new(database_url);
    opts.sqlx_logging(false);
    let db = Database::connect(opts).await?;

    Migrator::up(&db, None).await?;

    db.close().await?;
    Ok(())
}

async fn validate_schema(database_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut opts = ConnectOptions::new(database_url);
    opts.sqlx_logging(false);
    let db = Database::connect(opts).await?;

    // Verify core tables exist
    let tables = [
        "assessments",
        "normalization_audit_log",
        "outbox",
        "failed_records",
        "schema_violations",
    ];

    for table in &tables {
        let result = db
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "SELECT 1 FROM information_schema.tables WHERE table_name = '{table}'"
                ),
            ))
            .await?;

        if result.is_none() {
            return Err(format!("table '{table}' not found after migration").into());
        }
    }

    // Basic connectivity check
    db.execute_unprepared("SELECT 1").await?;

    db.close().await?;
    Ok(())
}
