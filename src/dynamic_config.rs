use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use tracing::{debug, info, warn};

use crate::config::AppConfig;
use crate::entity::dynamic_config;
use crate::state::AppState;

/// Spawn a background task that periodically polls the database for config changes
/// and applies them on top of the base (static) config.
pub fn spawn_refresh_task(state: Arc<AppState>, base_config: AppConfig) {
    tokio::spawn(async move {
        let mut last_updated_at: Option<DateTime<Utc>> = None;

        loop {
            // Read the current refresh interval from the live config (self-referential)
            let interval_secs = state.config().dynamic_config.refresh_interval_secs;
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;

            match load_all_entries(&state.db).await {
                Ok(entries) => {
                    if entries.is_empty() {
                        // No dynamic overrides — ensure we're running base config
                        if last_updated_at.is_some() {
                            info!("dynamic config: all overrides removed, reverting to base config");
                            state.update_config(base_config.clone());
                            last_updated_at = None;
                        }
                        continue;
                    }

                    // Check if anything changed via max(updated_at)
                    let max_ts = entries.iter().map(|e| e.updated_at).max();
                    if max_ts == last_updated_at {
                        debug!("dynamic config: no changes detected");
                        continue;
                    }

                    // Build override map and apply
                    let overrides: HashMap<String, serde_json::Value> = entries
                        .into_iter()
                        .map(|e| (e.key, e.value))
                        .collect();

                    let new_config = base_config.with_dynamic_overrides(&overrides);
                    state.update_config(new_config);
                    last_updated_at = max_ts;

                    info!("dynamic config: applied {} override(s)", overrides.len());
                }
                Err(e) => {
                    warn!(error = %e, "dynamic config: failed to load from database, keeping current config");
                }
            }
        }
    });
}

/// Load all dynamic config entries from the database.
async fn load_all_entries(
    db: &sea_orm::DatabaseConnection,
) -> Result<Vec<dynamic_config::Model>, sea_orm::DbErr> {
    dynamic_config::Entity::find().all(db).await
}

/// Get a single config entry by key.
pub async fn get_entry(
    db: &sea_orm::DatabaseConnection,
    key: &str,
) -> Result<Option<dynamic_config::Model>, sea_orm::DbErr> {
    dynamic_config::Entity::find_by_id(key.to_string())
        .one(db)
        .await
}

/// List all config entries.
pub async fn list_entries(
    db: &sea_orm::DatabaseConnection,
) -> Result<Vec<dynamic_config::Model>, sea_orm::DbErr> {
    dynamic_config::Entity::find().all(db).await
}

/// Set (upsert) a config entry.
pub async fn set_entry(
    db: &sea_orm::DatabaseConnection,
    key: &str,
    value: serde_json::Value,
    description: Option<String>,
    updated_by: &str,
) -> Result<dynamic_config::Model, sea_orm::DbErr> {
    let now = Utc::now();

    // Try to find existing
    let existing = dynamic_config::Entity::find_by_id(key.to_string())
        .one(db)
        .await?;

    if let Some(_existing) = existing {
        // Update
        let active = dynamic_config::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value),
            description: Set(description),
            updated_at: Set(now.into()),
            updated_by: Set(updated_by.to_string()),
        };
        active.update(db).await
    } else {
        // Insert
        let active = dynamic_config::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value),
            description: Set(description),
            updated_at: Set(now.into()),
            updated_by: Set(updated_by.to_string()),
        };
        active.insert(db).await
    }
}

/// Delete a config entry by key.
pub async fn delete_entry(
    db: &sea_orm::DatabaseConnection,
    key: &str,
) -> Result<bool, sea_orm::DbErr> {
    let result = dynamic_config::Entity::delete_many()
        .filter(dynamic_config::Column::Key.eq(key))
        .exec(db)
        .await?;

    Ok(result.rows_affected > 0)
}
