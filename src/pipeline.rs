use tracing::info;

use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;
use crate::provider::NormalizationProvider;

/// Staged normalization pipeline:
/// 1. Validate input (provider-specific)
/// 2. Normalize (parse + transform + scale scores)
/// 3. Validate output (unified schema invariants)
pub struct NormalizationPipeline;

impl NormalizationPipeline {
    pub fn run(
        provider: &dyn NormalizationProvider,
        raw: &[u8],
    ) -> Result<Vec<UnifiedAssessment>, AppError> {
        // Stage 1: Validate input
        provider.validate_input(raw)?;

        // Stage 2: Normalize (parse → transform → scale)
        let assessments = provider.normalize(raw)?;

        // Stage 3: Validate each output record
        for assessment in &assessments {
            assessment.validate()?;
        }

        info!(
            provider = provider.name(),
            count = assessments.len(),
            "pipeline completed"
        );

        Ok(assessments)
    }
}
