use std::collections::HashMap;

use serde::Deserialize;

/// Provider B: Flat key-value format
/// Scores are already on 0-100 scale. Dates are ISO 8601 strings.
/// All data is flat key-value pairs.
#[derive(Debug, Deserialize)]
pub struct ProviderBPayload {
    pub patient_id: String,
    pub assessment_date: String,
    pub assessment_type: String,
    pub scores: HashMap<String, f64>,
}
