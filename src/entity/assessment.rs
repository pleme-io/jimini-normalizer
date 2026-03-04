use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "assessments")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub patient_id: String,
    pub assessment_date: DateTimeUtc,
    pub assessment_type: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub scores: Json,
    pub source_provider: String,
    pub source_format: String,
    pub ingested_at: DateTimeUtc,
    pub version: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::normalization_audit_log::Entity")]
    AuditLog,
    #[sea_orm(has_many = "super::outbox::Entity")]
    Outbox,
}

impl Related<super::normalization_audit_log::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AuditLog.def()
    }
}

impl Related<super::outbox::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Outbox.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
