use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DynamicConfig::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DynamicConfig::Key)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DynamicConfig::Value).json_binary().not_null())
                    .col(ColumnDef::new(DynamicConfig::Description).string())
                    .col(
                        ColumnDef::new(DynamicConfig::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(DynamicConfig::UpdatedBy)
                            .string()
                            .not_null()
                            .default("system"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_dynamic_config_updated_at")
                    .table(DynamicConfig::Table)
                    .col(DynamicConfig::UpdatedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DynamicConfig::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum DynamicConfig {
    Table,
    Key,
    Value,
    Description,
    UpdatedAt,
    UpdatedBy,
}
