use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::dynamic_config;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub description: Option<String>,
    pub updated_at: String,
    pub updated_by: String,
}

#[derive(Deserialize)]
pub struct SetConfigRequest {
    pub value: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn list_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ConfigEntry>>, AppError> {
    let entries = dynamic_config::list_entries(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    let result: Vec<ConfigEntry> = entries
        .into_iter()
        .map(|e| ConfigEntry {
            key: e.key,
            value: e.value,
            description: e.description,
            updated_at: e.updated_at.to_rfc3339(),
            updated_by: e.updated_by,
        })
        .collect();

    Ok(Json(result))
}

pub async fn get_config(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<ConfigEntry>, AppError> {
    let entry = dynamic_config::get_entry(&state.db, &key)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("config key '{key}' not found")))?;

    Ok(Json(ConfigEntry {
        key: entry.key,
        value: entry.value,
        description: entry.description,
        updated_at: entry.updated_at.to_rfc3339(),
        updated_by: entry.updated_by,
    }))
}

pub async fn set_config(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(body): Json<SetConfigRequest>,
) -> Result<(StatusCode, Json<ConfigEntry>), AppError> {
    let entry = dynamic_config::set_entry(
        &state.db,
        &key,
        body.value,
        body.description,
        "admin-api",
    )
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(ConfigEntry {
            key: entry.key,
            value: entry.value,
            description: entry.description,
            updated_at: entry.updated_at.to_rfc3339(),
            updated_by: entry.updated_by,
        }),
    ))
}

pub async fn delete_config(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    let deleted = dynamic_config::delete_entry(&state.db, &key)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound(format!("config key '{key}' not found")))
    }
}
