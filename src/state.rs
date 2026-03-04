use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use async_nats::jetstream;
use sea_orm::DatabaseConnection;
use tracing::info;

use crate::config::AppConfig;
use crate::provider::ProviderRegistry;
use crate::sink::postgres::PostgresSink;
use crate::sink::SinkRegistry;

pub struct AppState {
    config: ArcSwap<AppConfig>,
    providers: ArcSwap<ProviderRegistry>,
    sinks: ArcSwap<SinkRegistry>,
    postgres_sink: PostgresSink,
    pub db: DatabaseConnection,
    pub js: jetstream::Context,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        db: DatabaseConnection,
        js: jetstream::Context,
    ) -> Arc<Self> {
        let providers = if config.enabled_providers.is_empty() {
            ProviderRegistry::new()
        } else {
            ProviderRegistry::with_filter(Some(&config.enabled_providers))
        };

        let sinks = if config.outbox.enabled_sinks.is_empty() {
            SinkRegistry::new(&config.sinks)
        } else {
            SinkRegistry::with_filter(&config.sinks, Some(&config.outbox.enabled_sinks))
        };

        let postgres_sink = PostgresSink::new(db.clone());

        Arc::new(Self {
            config: ArcSwap::from_pointee(config),
            providers: ArcSwap::from_pointee(providers),
            sinks: ArcSwap::from_pointee(sinks),
            postgres_sink,
            db,
            js,
        })
    }

    /// Load-free read of the current config.
    pub fn config(&self) -> Guard<Arc<AppConfig>> {
        self.config.load()
    }

    /// Load-free read of the current provider registry.
    pub fn providers(&self) -> Guard<Arc<ProviderRegistry>> {
        self.providers.load()
    }

    /// Load-free read of the current sink registry.
    pub fn sinks(&self) -> Guard<Arc<SinkRegistry>> {
        self.sinks.load()
    }

    /// Access the PostgresSink for ACID assessment persistence.
    pub fn postgres_sink(&self) -> &PostgresSink {
        &self.postgres_sink
    }

    /// Atomically swap the config. If `enabled_providers` changed, rebuild the
    /// provider registry. If `outbox.enabled_sinks` or `sinks` changed, rebuild
    /// the sink registry.
    pub fn update_config(&self, new_config: AppConfig) {
        let old = self.config.load();
        let providers_changed = old.enabled_providers != new_config.enabled_providers;
        let sinks_changed = old.outbox.enabled_sinks != new_config.outbox.enabled_sinks;

        if providers_changed {
            let new_providers = if new_config.enabled_providers.is_empty() {
                ProviderRegistry::new()
            } else {
                ProviderRegistry::with_filter(Some(&new_config.enabled_providers))
            };
            info!(
                providers = ?new_config.enabled_providers,
                "dynamic config: rebuilding provider registry"
            );
            self.providers.store(Arc::new(new_providers));
        }

        if sinks_changed {
            let new_sinks = if new_config.outbox.enabled_sinks.is_empty() {
                SinkRegistry::new(&new_config.sinks)
            } else {
                SinkRegistry::with_filter(
                    &new_config.sinks,
                    Some(&new_config.outbox.enabled_sinks),
                )
            };
            info!(
                enabled_sinks = ?new_config.outbox.enabled_sinks,
                "dynamic config: rebuilding sink registry"
            );
            self.sinks.store(Arc::new(new_sinks));
        }

        self.config.store(Arc::new(new_config));
    }
}
