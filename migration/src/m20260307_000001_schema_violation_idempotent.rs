use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Unique index on (boundary, provider, input_hash) prevents duplicate
        // schema violation records when NATS redelivers an error message.
        // The worker groups violations by boundary, so each (boundary, provider, input_hash)
        // tuple should only produce one row.
        manager
            .create_index(
                Index::create()
                    .name("idx_schema_violations_boundary_provider_hash_unique")
                    .table(SchemaViolations::Table)
                    .col(SchemaViolations::Boundary)
                    .col(SchemaViolations::Provider)
                    .col(SchemaViolations::InputHash)
                    .unique()
                    .if_not_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_schema_violations_boundary_provider_hash_unique")
                    .table(SchemaViolations::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum SchemaViolations {
    Table,
    Boundary,
    Provider,
    InputHash,
}
