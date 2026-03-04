use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add unique index on normalization_audit_log.assessment_id to prevent
        // duplicate audit entries when NATS redelivers a message. The worker
        // uses ON CONFLICT DO NOTHING to silently skip duplicates.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_assessment_id_unique")
                    .table(NormalizationAuditLog::Table)
                    .col(NormalizationAuditLog::AssessmentId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Add unique index on failed_records.input_hash + provider to prevent
        // duplicate failed records when NATS redelivers an error message.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_failed_records_input_hash_provider_unique")
                    .table(FailedRecords::Table)
                    .col(FailedRecords::InputHash)
                    .col(FailedRecords::Provider)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_audit_assessment_id_unique")
                    .table(NormalizationAuditLog::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_failed_records_input_hash_provider_unique")
                    .table(FailedRecords::Table)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum NormalizationAuditLog {
    Table,
    AssessmentId,
}

#[derive(DeriveIden)]
enum FailedRecords {
    Table,
    InputHash,
    Provider,
}
