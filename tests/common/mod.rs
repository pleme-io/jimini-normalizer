#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers::ContainerAsync;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::postgres::Postgres;

/// Shared test infrastructure managing Postgres + NATS containers.
pub struct TestInfra {
    pub db: DatabaseConnection,
    pub js: async_nats::jetstream::Context,
    pub nats_client: async_nats::Client,
    pub database_url: String,
    pub nats_url: String,
    // Hold containers alive for the duration of tests
    _pg_container: ContainerAsync<Postgres>,
    _nats_container: ContainerAsync<Nats>,
}

impl TestInfra {
    /// Spin up Postgres + NATS containers, run migrations, return test infrastructure.
    pub async fn start() -> Self {
        // Start containers in parallel
        let pg_handle = tokio::spawn(async {
            Postgres::default()
                .start()
                .await
                .expect("failed to start postgres container")
        });

        let nats_handle = tokio::spawn(async {
            Nats::default()
                .with_cmd(vec!["--jetstream"])
                .start()
                .await
                .expect("failed to start nats container")
        });

        let pg_container = pg_handle.await.expect("pg task panicked");
        let nats_container = nats_handle.await.expect("nats task panicked");

        // Get connection details
        let pg_host = pg_container.get_host().await.expect("pg host");
        let pg_port = pg_container.get_host_port_ipv4(5432).await.expect("pg port");
        let database_url = format!("postgres://postgres:postgres@{pg_host}:{pg_port}/postgres");

        let nats_host = nats_container.get_host().await.expect("nats host");
        let nats_port = nats_container.get_host_port_ipv4(4222).await.expect("nats port");
        let nats_url = format!("nats://{nats_host}:{nats_port}");

        // Connect to Postgres with retry
        let db = connect_with_retry(&database_url, 10).await;

        // Run migrations
        {
            use migration::{Migrator, MigratorTrait};
            Migrator::up(&db, None)
                .await
                .expect("failed to run migrations");
        }

        // Connect to NATS with retry
        let nats_client = connect_nats_with_retry(&nats_url, 10).await;
        let js = async_nats::jetstream::new(nats_client.clone());

        Self {
            db,
            js,
            nats_client,
            database_url,
            nats_url,
            _pg_container: pg_container,
            _nats_container: nats_container,
        }
    }

    /// Create an AppState pointing at the test containers.
    pub fn app_state(&self) -> Arc<jimini_normalizer::state::AppState> {
        let config = jimini_normalizer::config::AppConfig::default_test();
        jimini_normalizer::state::AppState::new(config, self.db.clone(), self.js.clone())
    }

    /// Create an AppState with custom config.
    pub fn app_state_with_config(
        &self,
        config: jimini_normalizer::config::AppConfig,
    ) -> Arc<jimini_normalizer::state::AppState> {
        jimini_normalizer::state::AppState::new(config, self.db.clone(), self.js.clone())
    }

    /// Load the test-config.yaml fixture with PII enabled, overriding
    /// database.url and nats.url to point at the testcontainers.
    pub fn load_test_config(&self) -> jimini_normalizer::config::AppConfig {
        use figment::providers::{Format, Serialized, Yaml};
        use figment::Figment;

        let mut cfg: jimini_normalizer::config::AppConfig =
            Figment::from(Serialized::defaults(jimini_normalizer::config::AppConfig::default()))
                .merge(Yaml::file("fixtures/test-config.yaml"))
                .extract()
                .expect("failed to load test-config.yaml");

        cfg.database.url = self.database_url.clone();
        cfg.nats.url = self.nats_url.clone();
        cfg
    }

    /// Create an AppState with the test-config.yaml fixture (PII enabled).
    pub fn app_state_with_pii(&self) -> Arc<jimini_normalizer::state::AppState> {
        let config = self.load_test_config();
        jimini_normalizer::state::AppState::new(config, self.db.clone(), self.js.clone())
    }
}

async fn connect_with_retry(url: &str, max_attempts: u32) -> DatabaseConnection {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut opts = ConnectOptions::new(url);
        opts.max_connections(5).sqlx_logging(false);

        match Database::connect(opts).await {
            Ok(db) => return db,
            Err(e) if attempt < max_attempts => {
                let delay = Duration::from_millis(100 * 2u64.pow(attempt.min(5)));
                eprintln!("DB connect attempt {attempt}/{max_attempts} failed: {e}, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
            }
            Err(e) => panic!("failed to connect to test database after {max_attempts} attempts: {e}"),
        }
    }
}

async fn connect_nats_with_retry(url: &str, max_attempts: u32) -> async_nats::Client {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match async_nats::connect(url).await {
            Ok(client) => return client,
            Err(e) if attempt < max_attempts => {
                let delay = Duration::from_millis(100 * 2u64.pow(attempt.min(5)));
                eprintln!("NATS connect attempt {attempt}/{max_attempts} failed: {e}, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
            }
            Err(e) => panic!("failed to connect to test NATS after {max_attempts} attempts: {e}"),
        }
    }
}
