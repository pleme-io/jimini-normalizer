use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "failed_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub provider: String,
    #[sea_orm(column_type = "Text")]
    pub raw_input: String, // original input preserved for replay
    pub error_kind: String, // "parse" | "validation" | "schema_mismatch" | "internal"
    #[sea_orm(column_type = "JsonBinary")]
    pub error_detail: Json, // structured error info
    pub input_hash: String,
    pub replayed: bool,
    pub replayed_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
