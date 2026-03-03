use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Provider A: Nested JSON format
/// Scores are on a 0-10 scale, dates are ISO 8601.
#[derive(Debug, Deserialize)]
pub struct ProviderAPayload {
    pub patient: PatientInfo,
    pub assessment: AssessmentInfo,
}

#[derive(Debug, Deserialize)]
pub struct PatientInfo {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct AssessmentInfo {
    pub date: DateTime<Utc>,
    #[serde(rename = "type")]
    pub assessment_type: String,
    pub scores: Vec<ProviderAScore>,
}

#[derive(Debug, Deserialize)]
pub struct ProviderAScore {
    pub category: String,
    pub value: f64,
}
