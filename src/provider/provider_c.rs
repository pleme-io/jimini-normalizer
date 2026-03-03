use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use uuid::Uuid;

use crate::error::AppError;
use crate::model::provider_c::ProviderCRow;
use crate::model::unified::{AssessmentMetadata, Score, UnifiedAssessment};
use crate::provider::NormalizationProvider;

/// Provider C: CSV format with scores on a 0-10 scale.
/// Multiple rows may belong to the same patient/date/type combination.
pub struct ProviderC;

impl NormalizationProvider for ProviderC {
    fn name(&self) -> &str {
        "provider_c"
    }

    fn format(&self) -> &str {
        "csv"
    }

    fn validate_input(&self, raw: &[u8]) -> Result<(), AppError> {
        let mut reader = csv::Reader::from_reader(raw);
        for result in reader.deserialize::<ProviderCRow>() {
            result.map_err(|e| AppError::ParseError(e.to_string()))?;
        }
        Ok(())
    }

    fn normalize(&self, raw: &[u8]) -> Result<Vec<UnifiedAssessment>, AppError> {
        let mut reader = csv::Reader::from_reader(raw);
        let rows: Vec<ProviderCRow> = reader
            .deserialize()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::ParseError(e.to_string()))?;

        // Group rows by (patient_id, date, assessment_type)
        let mut groups: HashMap<(String, String, String), Vec<ProviderCRow>> = HashMap::new();
        for row in rows {
            let key = (
                row.patient_id.clone(),
                row.date.clone(),
                row.assessment_type.clone(),
            );
            groups.entry(key).or_default().push(row);
        }

        let now = Utc::now();
        let mut assessments = Vec::new();

        for ((patient_id, date_str, assessment_type), rows) in groups {
            // Parse MM/DD/YYYY date format
            let naive = NaiveDate::parse_from_str(&date_str, "%m/%d/%Y")
                .map_err(|e| AppError::ParseError(format!("invalid date '{date_str}': {e}")))?;
            let assessment_date = naive
                .and_hms_opt(0, 0, 0)
                .expect("valid time")
                .and_utc();

            let scores = rows
                .into_iter()
                .map(|r| Score {
                    dimension: r.dimension,
                    value: r.score * 10.0, // 0-10 → 0-100
                    scale: "0-100".to_string(),
                })
                .collect();

            assessments.push(UnifiedAssessment {
                id: Uuid::new_v4(),
                patient_id,
                assessment_date,
                assessment_type,
                scores,
                metadata: AssessmentMetadata {
                    source_provider: self.name().to_string(),
                    source_format: self.format().to_string(),
                    ingested_at: now,
                    version: "1.0".to_string(),
                },
            });
        }

        Ok(assessments)
    }
}
