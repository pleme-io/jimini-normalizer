use serde::Deserialize;

/// Provider C: CSV format
/// Each row is a separate assessment score line.
/// Scores are on a 0-10 scale. Dates are MM/DD/YYYY format.
#[derive(Debug, Deserialize)]
pub struct ProviderCRow {
    pub patient_id: String,
    pub date: String,
    pub assessment_type: String,
    pub dimension: String,
    pub score: f64,
}
