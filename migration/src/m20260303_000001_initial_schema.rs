use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Assessments: normalized records persisted from the pipeline
        manager
            .create_table(
                Table::create()
                    .table(Assessments::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Assessments::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Assessments::PatientId).text().not_null())
                    .col(ColumnDef::new(Assessments::AssessmentDate).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Assessments::AssessmentType).text().not_null())
                    .col(ColumnDef::new(Assessments::Scores).json_binary().not_null())
                    .col(ColumnDef::new(Assessments::SourceProvider).text().not_null())
                    .col(ColumnDef::new(Assessments::SourceFormat).text().not_null())
                    .col(ColumnDef::new(Assessments::IngestedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Assessments::Version).text().not_null().default("1.0"))
                    .col(
                        ColumnDef::new(Assessments::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_assessments_patient_id")
                    .table(Assessments::Table)
                    .col(Assessments::PatientId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_assessments_provider")
                    .table(Assessments::Table)
                    .col(Assessments::SourceProvider)
                    .to_owned(),
            )
            .await?;

        // Normalization audit log: records what transformations were applied
        manager
            .create_table(
                Table::create()
                    .table(NormalizationAuditLog::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(NormalizationAuditLog::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(NormalizationAuditLog::AssessmentId).uuid().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::Provider).text().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::SourceFormat).text().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::InputHash).text().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::InputSizeBytes).integer().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::ScoreCount).integer().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::Transformations).json_binary().not_null())
                    .col(ColumnDef::new(NormalizationAuditLog::DurationUs).big_integer().not_null())
                    .col(
                        ColumnDef::new(NormalizationAuditLog::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(NormalizationAuditLog::Table, NormalizationAuditLog::AssessmentId)
                            .to(Assessments::Table, Assessments::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_assessment_id")
                    .table(NormalizationAuditLog::Table)
                    .col(NormalizationAuditLog::AssessmentId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_provider")
                    .table(NormalizationAuditLog::Table)
                    .col(NormalizationAuditLog::Provider)
                    .to_owned(),
            )
            .await?;

        // Schema violations: boundary mismatches captured for auditing
        manager
            .create_table(
                Table::create()
                    .table(SchemaViolations::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SchemaViolations::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(SchemaViolations::Boundary).text().not_null()) // "pre_transform" | "post_transform"
                    .col(ColumnDef::new(SchemaViolations::Provider).text().not_null())
                    .col(ColumnDef::new(SchemaViolations::InputHash).text().not_null())
                    .col(ColumnDef::new(SchemaViolations::Violations).json_binary().not_null()) // [{field, expected, actual, message}]
                    .col(ColumnDef::new(SchemaViolations::RawInput).text().not_null()) // full input for replay
                    .col(
                        ColumnDef::new(SchemaViolations::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_schema_violations_boundary")
                    .table(SchemaViolations::Table)
                    .col(SchemaViolations::Boundary)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_schema_violations_provider")
                    .table(SchemaViolations::Table)
                    .col(SchemaViolations::Provider)
                    .to_owned(),
            )
            .await?;

        // Outbox: mailbox pattern — holds normalized assessments pending flush to sink
        manager
            .create_table(
                Table::create()
                    .table(Outbox::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Outbox::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Outbox::AssessmentId).uuid().not_null())
                    .col(ColumnDef::new(Outbox::Payload).json_binary().not_null()) // full UnifiedAssessment JSON
                    .col(ColumnDef::new(Outbox::Status).text().not_null().default("pending")) // pending | flushed | failed
                    .col(ColumnDef::new(Outbox::Retries).integer().not_null().default(0))
                    .col(
                        ColumnDef::new(Outbox::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Outbox::FlushedAt).timestamp_with_time_zone().null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(Outbox::Table, Outbox::AssessmentId)
                            .to(Assessments::Table, Assessments::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_outbox_status")
                    .table(Outbox::Table)
                    .col(Outbox::Status)
                    .to_owned(),
            )
            .await?;

        // Failed records: all failure data preserved for exploration and replay
        manager
            .create_table(
                Table::create()
                    .table(FailedRecords::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(FailedRecords::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(FailedRecords::Provider).text().not_null())
                    .col(ColumnDef::new(FailedRecords::RawInput).text().not_null()) // original input for replay
                    .col(ColumnDef::new(FailedRecords::ErrorKind).text().not_null()) // "parse" | "validation" | "schema_mismatch" | "internal"
                    .col(ColumnDef::new(FailedRecords::ErrorDetail).json_binary().not_null()) // structured error info
                    .col(ColumnDef::new(FailedRecords::InputHash).text().not_null())
                    .col(ColumnDef::new(FailedRecords::Replayed).boolean().not_null().default(false))
                    .col(ColumnDef::new(FailedRecords::ReplayedAt).timestamp_with_time_zone().null())
                    .col(
                        ColumnDef::new(FailedRecords::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_failed_records_provider")
                    .table(FailedRecords::Table)
                    .col(FailedRecords::Provider)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_failed_records_error_kind")
                    .table(FailedRecords::Table)
                    .col(FailedRecords::ErrorKind)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_failed_records_replayed")
                    .table(FailedRecords::Table)
                    .col(FailedRecords::Replayed)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop in FK dependency order (children first)
        manager.drop_table(Table::drop().table(FailedRecords::Table).if_exists().to_owned()).await?;
        manager.drop_table(Table::drop().table(Outbox::Table).if_exists().to_owned()).await?;
        manager.drop_table(Table::drop().table(SchemaViolations::Table).if_exists().to_owned()).await?;
        manager.drop_table(Table::drop().table(NormalizationAuditLog::Table).if_exists().to_owned()).await?;
        manager.drop_table(Table::drop().table(Assessments::Table).if_exists().to_owned()).await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Assessments {
    Table,
    Id,
    PatientId,
    AssessmentDate,
    AssessmentType,
    Scores,
    SourceProvider,
    SourceFormat,
    IngestedAt,
    Version,
    CreatedAt,
}

#[derive(DeriveIden)]
enum NormalizationAuditLog {
    Table,
    Id,
    AssessmentId,
    Provider,
    SourceFormat,
    InputHash,
    InputSizeBytes,
    ScoreCount,
    Transformations,
    DurationUs,
    CreatedAt,
}

#[derive(DeriveIden)]
enum SchemaViolations {
    Table,
    Id,
    Boundary,
    Provider,
    InputHash,
    Violations,
    RawInput,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Outbox {
    Table,
    Id,
    AssessmentId,
    Payload,
    Status,
    Retries,
    CreatedAt,
    FlushedAt,
}

#[derive(DeriveIden)]
enum FailedRecords {
    Table,
    Id,
    Provider,
    RawInput,
    ErrorKind,
    ErrorDetail,
    InputHash,
    Replayed,
    ReplayedAt,
    CreatedAt,
}
