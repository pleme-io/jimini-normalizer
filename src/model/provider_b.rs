use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

/// Provider B: Flat key-value format
/// Scores are embedded as `score_*` prefixed keys at the top level.
/// Scores are already on 0-100 scale. Dates are ISO 8601 strings.
/// Includes PHI fields (patient_name) that must not be logged.
#[derive(Debug, Deserialize)]
pub struct ProviderBPayload {
    pub patient_id: String,
    #[serde(default)]
    pub patient_name: Option<String>,
    pub assessment_type: String,
    #[serde(default)]
    pub notes: Option<String>,
    /// Captures all remaining fields — we extract score_* keys from this.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl ProviderBPayload {
    /// Extract score dimensions from `score_*` prefixed keys.
    pub fn extract_scores(&self) -> HashMap<String, f64> {
        self.extra
            .iter()
            .filter_map(|(k, v)| {
                if let Some(dimension) = k.strip_prefix("score_") {
                    v.as_f64().map(|val| (dimension.to_string(), val))
                } else {
                    None
                }
            })
            .collect()
    }
}
