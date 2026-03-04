use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "outbox")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub assessment_id: Uuid,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: Json, // full UnifiedAssessment JSON
    pub status: String,  // "pending" | "flushed" | "failed"
    pub retries: i32,
    pub created_at: DateTimeUtc,
    pub flushed_at: Option<DateTimeUtc>,
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
