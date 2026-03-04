//! Audit checker mode — runs as a CronJob every 10 minutes.
//!
//! Queries the database for anomalies:
//! - Unreplayed failed records (by provider, error_kind)
//! - Recent schema violations (by boundary)
//! - Stuck or failed outbox messages
//!
//! Logs findings as structured JSON. Exits non-zero if critical issues found.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::{error, info, warn};

use crate::entity::{failed_record, outbox, schema_violation};

/// Threshold for unreplayed failed records before alerting as critical
const FAILED_RECORDS_CRITICAL: usize = 10;

/// Threshold for stuck outbox messages before alerting as critical
const OUTBOX_STUCK_CRITICAL: usize = 5;

/// Run the audit checker. Returns exit code: 0 = OK, 1 = critical issues found, 2 = DB error.
pub async fn run(db: &DatabaseConnection) -> i32 {
    info!("audit_checker: starting database audit scan");

    let mut critical = false;

    // 1. Check unreplayed failed records
    match check_failed_records(db).await {
        Ok(count) => {
            if count > 0 {
                if count >= FAILED_RECORDS_CRITICAL {
                    error!(
                        count,
                        threshold = FAILED_RECORDS_CRITICAL,
                        "audit_checker: CRITICAL — unreplayed failed records exceed threshold"
                    );
                    critical = true;
                } else {
                    warn!(count, "audit_checker: unreplayed failed records found");
                }
            } else {
                info!("audit_checker: no unreplayed failed records");
            }
        }
        Err(e) => {
            error!(error = %e, "audit_checker: failed to query failed_records");
            return 2;
        }
    }

    // 2. Check recent schema violations (last hour)
    match check_schema_violations(db).await {
        Ok(count) => {
            if count > 0 {
                warn!(count, "audit_checker: schema violations in last hour");
                // Schema violations are always noteworthy but not necessarily critical
                // (they may be expected during provider changes)
            } else {
                info!("audit_checker: no recent schema violations");
            }
        }
        Err(e) => {
            error!(error = %e, "audit_checker: failed to query schema_violations");
            return 2;
        }
    }

    // 3. Check stuck/failed outbox messages
    match check_outbox(db).await {
        Ok((pending, failed)) => {
            if failed > 0 {
                error!(
                    failed_count = failed,
                    "audit_checker: failed outbox messages found"
                );
                critical = true;
            }
            if pending >= OUTBOX_STUCK_CRITICAL {
                warn!(
                    pending_count = pending,
                    threshold = OUTBOX_STUCK_CRITICAL,
                    "audit_checker: high number of pending outbox messages"
                );
            }
            if pending == 0 && failed == 0 {
                info!("audit_checker: outbox is healthy");
            }
        }
        Err(e) => {
            error!(error = %e, "audit_checker: failed to query outbox");
            return 2;
        }
    }

    if critical {
        error!("audit_checker: CRITICAL issues found — see above");
        1
    } else {
        info!("audit_checker: scan complete, no critical issues");
        0
    }
}

async fn check_failed_records(db: &DatabaseConnection) -> Result<usize, sea_orm::DbErr> {
    let records = failed_record::Entity::find()
        .filter(failed_record::Column::Replayed.eq(false))
        .all(db)
        .await?;

    // Log breakdown by provider and error_kind
    if !records.is_empty() {
        let mut by_provider: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut by_kind: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for r in &records {
            *by_provider.entry(r.provider.clone()).or_default() += 1;
            *by_kind.entry(r.error_kind.clone()).or_default() += 1;
        }
        for (provider, count) in &by_provider {
            warn!(provider, count, "audit_checker: unreplayed failures by provider");
        }
        for (kind, count) in &by_kind {
            warn!(error_kind = kind, count, "audit_checker: unreplayed failures by kind");
        }
    }

    Ok(records.len())
}

async fn check_schema_violations(db: &DatabaseConnection) -> Result<usize, sea_orm::DbErr> {
    let one_hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    let violations = schema_violation::Entity::find()
        .filter(schema_violation::Column::CreatedAt.gte(one_hour_ago))
        .all(db)
        .await?;

    if !violations.is_empty() {
        let mut by_boundary: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut by_provider: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for v in &violations {
            *by_boundary.entry(v.boundary.clone()).or_default() += 1;
            *by_provider.entry(v.provider.clone()).or_default() += 1;
        }
        for (boundary, count) in &by_boundary {
            warn!(boundary, count, "audit_checker: violations by boundary (last hour)");
        }
        for (provider, count) in &by_provider {
            warn!(provider, count, "audit_checker: violations by provider (last hour)");
        }
    }

    Ok(violations.len())
}

async fn check_outbox(db: &DatabaseConnection) -> Result<(usize, usize), sea_orm::DbErr> {
    let pending = outbox::Entity::find()
        .filter(outbox::Column::Status.eq("pending"))
        .all(db)
        .await?
        .len();

    let failed = outbox::Entity::find()
        .filter(outbox::Column::Status.eq("failed"))
        .all(db)
        .await?
        .len();

    Ok((pending, failed))
}
