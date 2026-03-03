use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Provider A: Nested JSON format
/// Scores are on a 0-10 scale as a flat object, dates are ISO 8601.
/// Includes PHI fields (name, dob) that must not be logged.
#[derive(Debug, Deserialize)]
pub struct ProviderAPayload {
    pub patient: PatientInfo,
    pub assessment: AssessmentInfo,
}

#[derive(Debug, Deserialize)]
pub struct PatientInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub dob: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AssessmentInfo {
    #[serde(default)]
    pub date: Option<DateTime<Utc>>,
    #[serde(rename = "type")]
    pub assessment_type: String,
    pub scores: HashMap<String, f64>,
    #[serde(default)]
    pub notes: Option<String>,
}
