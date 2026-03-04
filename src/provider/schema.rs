//! Provider schema definition for building new providers declaratively.
//!
//! A `ProviderSchema` captures the common normalization patterns:
//! - How to extract patient_id from the input
//! - How to extract scores and what scale they use
//! - How to map to assessment_type
//!
//! For simple JSON providers, implement `JsonProvider` instead of the full
//! `NormalizationProvider` trait to avoid boilerplate.

use serde::Deserialize;

/// Describes how a provider's scores are structured.
#[derive(Debug, Clone)]
pub struct ScoreSchema {
    /// Scale range of incoming scores (e.g., 0-10, 0-5, 0-100).
    pub input_min: f64,
    pub input_max: f64,
    /// Target scale for unified output (always 0-100).
    pub output_min: f64,
    pub output_max: f64,
}

impl ScoreSchema {
    /// 0-10 scale (common for clinical assessments).
    pub fn scale_0_10() -> Self {
        Self {
            input_min: 0.0,
            input_max: 10.0,
            output_min: 0.0,
            output_max: 100.0,
        }
    }

    /// 0-5 scale (Likert-style).
    pub fn scale_0_5() -> Self {
        Self {
            input_min: 0.0,
            input_max: 5.0,
            output_min: 0.0,
            output_max: 100.0,
        }
    }

    /// Already normalized 0-100 scale (no transformation needed).
    pub fn scale_0_100() -> Self {
        Self {
            input_min: 0.0,
            input_max: 100.0,
            output_min: 0.0,
            output_max: 100.0,
        }
    }

    /// Linear rescale a value from input range to output range.
    pub fn rescale(&self, value: f64) -> f64 {
        let clamped = value.clamp(self.input_min, self.input_max);
        let normalized = (clamped - self.input_min) / (self.input_max - self.input_min);
        self.output_min + normalized * (self.output_max - self.output_min)
    }
}

/// Trait for JSON-based providers that follow the common pattern of:
/// 1. Deserialize to a typed struct
/// 2. Extract patient_id, assessment_type, scores from the struct
/// 3. Scale scores to 0-100
///
/// Implement this instead of `NormalizationProvider` for simpler providers.
/// The blanket impl on `NormalizationProvider` handles the rest.
pub trait JsonProvider: Send + Sync {
    /// The deserialization target type for this provider's input.
    type Payload: for<'de> Deserialize<'de>;

    /// Unique provider name.
    fn name(&self) -> &str;

    /// Format description.
    fn format(&self) -> &str;

    /// Score schema describing the input scale.
    fn score_schema(&self) -> ScoreSchema;

    /// Extract patient_id from the deserialized payload.
    fn patient_id(&self, payload: &Self::Payload) -> String;

    /// Extract assessment_type from the deserialized payload.
    fn assessment_type(&self, payload: &Self::Payload) -> String;

    /// Extract assessment date from the deserialized payload.
    /// Returns None to use the current timestamp.
    fn assessment_date(&self, payload: &Self::Payload) -> Option<chrono::DateTime<chrono::Utc>>;

    /// Extract raw score dimensions from the deserialized payload.
    /// Values should be on the input scale (before rescaling).
    fn raw_scores(&self, payload: &Self::Payload) -> Vec<(String, f64)>;
}

impl<T: JsonProvider> super::NormalizationProvider for T {
    fn name(&self) -> &str {
        JsonProvider::name(self)
    }

    fn format(&self) -> &str {
        JsonProvider::format(self)
    }

    fn validate_input(&self, raw: &[u8]) -> Result<(), crate::error::AppError> {
        serde_json::from_slice::<T::Payload>(raw)
            .map(|_| ())
            .map_err(|e| crate::error::AppError::ParseError(e.to_string()))
    }

    fn normalize(&self, raw: &[u8]) -> Result<Vec<crate::model::unified::UnifiedAssessment>, crate::error::AppError> {
        let payload: T::Payload =
            serde_json::from_slice(raw).map_err(|e| crate::error::AppError::ParseError(e.to_string()))?;

        let now = chrono::Utc::now();
        let schema = self.score_schema();

        let scores = self
            .raw_scores(&payload)
            .into_iter()
            .map(|(dimension, value)| crate::model::unified::Score {
                dimension,
                value: schema.rescale(value),
                scale: "0-100".to_string(),
            })
            .collect();

        let assessment = crate::model::unified::UnifiedAssessment {
            id: uuid::Uuid::new_v4(),
            patient_id: self.patient_id(&payload),
            assessment_date: self.assessment_date(&payload).unwrap_or(now),
            assessment_type: self.assessment_type(&payload),
            scores,
            metadata: crate::model::unified::AssessmentMetadata {
                source_provider: JsonProvider::name(self).to_string(),
                source_format: JsonProvider::format(self).to_string(),
                ingested_at: now,
                version: "1.0".to_string(),
            },
        };

        Ok(vec![assessment])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_0_10_rescales_correctly() {
        let schema = ScoreSchema::scale_0_10();
        assert!((schema.rescale(0.0) - 0.0).abs() < f64::EPSILON);
        assert!((schema.rescale(5.0) - 50.0).abs() < f64::EPSILON);
        assert!((schema.rescale(10.0) - 100.0).abs() < f64::EPSILON);
        assert!((schema.rescale(7.0) - 70.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_0_5_rescales_correctly() {
        let schema = ScoreSchema::scale_0_5();
        assert!((schema.rescale(0.0) - 0.0).abs() < f64::EPSILON);
        assert!((schema.rescale(2.5) - 50.0).abs() < f64::EPSILON);
        assert!((schema.rescale(5.0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_0_100_is_identity() {
        let schema = ScoreSchema::scale_0_100();
        assert!((schema.rescale(42.0) - 42.0).abs() < f64::EPSILON);
        assert!((schema.rescale(100.0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rescale_clamps_out_of_range() {
        let schema = ScoreSchema::scale_0_10();
        assert!((schema.rescale(-5.0) - 0.0).abs() < f64::EPSILON);
        assert!((schema.rescale(15.0) - 100.0).abs() < f64::EPSILON);
    }
}
