use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use tracing::warn;
use uuid::Uuid;

use crate::audit::hash_input;
use crate::entity::failed_record;
use crate::error::AppError;
use crate::pii::{self, PiiConfig};

/// Persist a failed normalization attempt.
///
/// Raw input handling depends on PII config:
/// - `internal.store_raw_for_replay: true` → stores original input (for replay)
/// - `internal.store_raw_for_replay: false` → stores hash or "[REDACTED]" only
pub async fn record_failure(
    db: &DatabaseConnection,
    provider: &str,
    raw_input: &[u8],
    error: &AppError,
    pii_config: &PiiConfig,
) -> Result<Uuid, sea_orm::DbErr> {
    let (error_kind, error_detail) = classify_error(error);

    let id = Uuid::new_v4();
    let model = failed_record::ActiveModel {
        id: Set(id),
        provider: Set(provider.to_string()),
        raw_input: Set(pii::process_raw_input(raw_input, pii_config)),
        error_kind: Set(error_kind),
        error_detail: Set(error_detail),
        input_hash: Set(hash_input(raw_input)),
        replayed: Set(false),
        replayed_at: Set(None),
        created_at: Set(Utc::now().into()),
    };

    model.insert(db).await?;
    warn!(failed_record_id = %id, provider, "failure recorded for audit");
    Ok(id)
}

fn classify_error(error: &AppError) -> (String, serde_json::Value) {
    match error {
        AppError::Validation(fields) => (
            "validation".to_string(),
            serde_json::json!({
                "type": "validation",
                "fields": fields.iter().map(|f| {
                    serde_json::json!({"field": f.field, "message": f.message})
                }).collect::<Vec<_>>()
            }),
        ),
        AppError::ParseError(msg) => (
            "parse".to_string(),
            serde_json::json!({"type": "parse", "message": msg}),
        ),
        AppError::UnknownProvider(name) => (
            "schema_mismatch".to_string(),
            serde_json::json!({"type": "unknown_provider", "provider": name}),
        ),
        AppError::NotFound(msg) => (
            "internal".to_string(),
            serde_json::json!({"type": "not_found", "message": msg}),
        ),
        AppError::Internal(msg) => (
            "internal".to_string(),
            serde_json::json!({"type": "internal", "message": msg}),
        ),
        AppError::Database(msg) => (
            "internal".to_string(),
            serde_json::json!({"type": "database", "message": msg}),
        ),
    }
}

/// Mark a failed record as replayed after successful re-processing.
pub async fn mark_replayed(
    db: &DatabaseConnection,
    id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    let record = failed_record::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or(sea_orm::DbErr::RecordNotFound(format!("failed_record {id}")))?;

    let mut active: failed_record::ActiveModel = record.into();
    active.replayed = Set(true);
    active.replayed_at = Set(Some(Utc::now().into()));
    active.update(db).await?;
    Ok(())
}
