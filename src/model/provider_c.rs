use serde::Deserialize;

/// Provider C: CSV format
/// Each row is a separate metric line.
/// Scores are on a 0-10 scale. Dates are YYYY-MM-DD format.
/// Column names match the PDF: patient_id, assessment_date, metric_name, metric_value, category
#[derive(Debug, Deserialize)]
pub struct ProviderCRow {
    pub patient_id: String,
    pub assessment_date: String,
    pub metric_name: String,
    pub metric_value: f64,
    pub category: String,
}
