pub use sea_orm_migration::prelude::*;

mod m20260303_000001_initial_schema;
mod m20260304_000001_idempotency;
mod m20260305_000001_dynamic_config;
mod m20260306_000001_outbox_assessment_unique;
mod m20260307_000001_schema_violation_idempotent;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260303_000001_initial_schema::Migration),
            Box::new(m20260304_000001_idempotency::Migration),
            Box::new(m20260305_000001_dynamic_config::Migration),
            Box::new(m20260306_000001_outbox_assessment_unique::Migration),
            Box::new(m20260307_000001_schema_violation_idempotent::Migration),
        ]
    }
}
