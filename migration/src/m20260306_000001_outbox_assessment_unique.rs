use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add unique index on outbox.assessment_id to prevent duplicate entries
        // when NATS redelivers a message. The outbox worker uses the DB entry
        // for audit/observability only — the NATS stream is the primary delivery
        // mechanism.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_outbox_assessment_id_unique")
                    .table(Outbox::Table)
                    .col(Outbox::AssessmentId)
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
                    .name("idx_outbox_assessment_id_unique")
                    .table(Outbox::Table)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Outbox {
    Table,
    AssessmentId,
}
