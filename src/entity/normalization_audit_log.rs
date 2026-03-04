use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "normalization_audit_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub assessment_id: Uuid,
    pub provider: String,
    pub source_format: String,
    pub input_hash: String,
    pub input_size_bytes: i32,
    pub score_count: i32,
    #[sea_orm(column_type = "JsonBinary")]
    pub transformations: Json,
    pub duration_us: i64,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::assessment::Entity",
        from = "Column::AssessmentId",
        to = "super::assessment::Column::Id"
    )]
    Assessment,
}

impl Related<super::assessment::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Assessment.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
