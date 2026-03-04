use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "schema_violations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub boundary: String, // "pre_transform" | "post_transform"
    pub provider: String,
    pub input_hash: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub violations: Json, // [{field, expected, actual, message}]
    #[sea_orm(column_type = "Text")]
    pub raw_input: String, // full input for replay
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
