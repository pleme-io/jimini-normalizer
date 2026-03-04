use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::info;

use crate::entity::{
    assessment, failed_record, normalization_audit_log, outbox, schema_violation,
};

/// Purge records older than `retention_days` across all tables.
/// Designed to run as a K8s CronJob or on-demand cleanup.
pub async fn purge_expired(
    db: &DatabaseConnection,
    retention_days: i64,
) -> Result<PurgeReport, sea_orm::DbErr> {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days);
    let cutoff_dt: sea_orm::prelude::DateTimeUtc = cutoff;

    // Delete in dependency order: children first, then parents

    // 1. Outbox (references assessments)
    let outbox_result = outbox::Entity::delete_many()
        .filter(outbox::Column::CreatedAt.lt(cutoff_dt))
        .exec(db)
        .await?;

    // 2. Audit log (references assessments)
    let audit_result = normalization_audit_log::Entity::delete_many()
        .filter(normalization_audit_log::Column::CreatedAt.lt(cutoff_dt))
        .exec(db)
        .await?;

    // 3. Schema violations (no FK dependencies)
    let violations_result = schema_violation::Entity::delete_many()
        .filter(schema_violation::Column::CreatedAt.lt(cutoff_dt))
        .exec(db)
        .await?;

    // 4. Failed records (no FK dependencies)
    let failed_result = failed_record::Entity::delete_many()
        .filter(failed_record::Column::CreatedAt.lt(cutoff_dt))
        .exec(db)
        .await?;

    // 5. Assessments (parent — safe to delete after children removed)
    let assessments_result = assessment::Entity::delete_many()
        .filter(assessment::Column::CreatedAt.lt(cutoff_dt))
        .exec(db)
        .await?;

    let report = PurgeReport {
        retention_days,
        assessments_deleted: assessments_result.rows_affected,
        audit_logs_deleted: audit_result.rows_affected,
        violations_deleted: violations_result.rows_affected,
        failed_records_deleted: failed_result.rows_affected,
        outbox_deleted: outbox_result.rows_affected,
    };

    info!(
        retention_days,
        assessments = report.assessments_deleted,
        audit_logs = report.audit_logs_deleted,
        violations = report.violations_deleted,
        failed_records = report.failed_records_deleted,
        outbox = report.outbox_deleted,
        "retention purge completed"
    );

    Ok(report)
}

#[derive(Debug, serde::Serialize)]
pub struct PurgeReport {
    pub retention_days: i64,
    pub assessments_deleted: u64,
    pub audit_logs_deleted: u64,
    pub violations_deleted: u64,
    pub failed_records_deleted: u64,
    pub outbox_deleted: u64,
}
