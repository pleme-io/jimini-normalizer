use std::time::Instant;

use tracing::info;

use crate::audit::AuditBuilder;
use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;
use crate::pii::{self, PiiConfig};
use crate::provider::NormalizationProvider;
use crate::schema::{self, ViolationDetail};

pub struct PipelineResult {
    pub assessments: Vec<UnifiedAssessment>,
    pub audit: AuditBuilder,
    /// Pre-transform schema violations (may be empty if input was clean)
    pub pre_violations: Vec<ViolationDetail>,
    /// Post-transform schema violations per assessment (may be empty if output was clean)
    pub post_violations: Vec<ViolationDetail>,
}

impl std::fmt::Debug for PipelineResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineResult")
            .field("assessments_count", &self.assessments.len())
            .field("pre_violations", &self.pre_violations.len())
            .field("post_violations", &self.post_violations.len())
            .finish()
    }
}

/// Staged normalization pipeline:
/// 1. Pre-transform boundary validation (per-provider schema)
/// 2. Validate input (provider-specific deep validation)
/// 3. Normalize (parse + transform + scale scores)
/// 4. Post-transform boundary validation (centralized unified schema)
/// 5. Validate output (unified schema invariants)
pub struct NormalizationPipeline;

impl NormalizationPipeline {
    pub fn run(
        provider: &dyn NormalizationProvider,
        raw: &[u8],
    ) -> Result<PipelineResult, PipelineError> {
        Self::run_with_pii(provider, raw, None)
    }

    /// Run the pipeline with PII scrubbing applied to output assessments.
    /// When `pii_config` is Some, patient_id is scrubbed before leaving the pipeline.
    pub fn run_with_pii(
        provider: &dyn NormalizationProvider,
        raw: &[u8],
        pii_config: Option<&PiiConfig>,
    ) -> Result<PipelineResult, PipelineError> {
        let mut audit = AuditBuilder::new(raw);

        // Stage 1: Pre-transform boundary validation (per-source schema)
        let step_start = Instant::now();
        let pre_violations = schema::validate_pre_transform(provider.name(), raw);
        audit.record_step(
            "validate_pre_transform",
            step_start,
            serde_json::json!({
                "violations": pre_violations.len(),
                "passed": pre_violations.is_empty()
            }),
        );

        if !pre_violations.is_empty() {
            return Err(PipelineError {
                app_error: schema::violations_to_app_error(&pre_violations),
                audit,
                pre_violations,
                post_violations: vec![],
            });
        }

        // Stage 2: Validate input (provider-specific deep validation)
        let step_start = Instant::now();
        if let Err(e) = provider.validate_input(raw) {
            audit.record_step(
                "validate_input",
                step_start,
                serde_json::json!({"passed": false}),
            );
            return Err(PipelineError {
                app_error: e,
                audit,
                pre_violations: vec![],
                post_violations: vec![],
            });
        }
        audit.record_step(
            "validate_input",
            step_start,
            serde_json::json!({"passed": true}),
        );

        // Stage 3: Normalize (parse → transform → scale)
        let step_start = Instant::now();
        let assessments = match provider.normalize(raw) {
            Ok(a) => {
                audit.record_step(
                    "normalize",
                    step_start,
                    serde_json::json!({
                        "scores_produced": a.iter().map(|x| x.scores.len()).sum::<usize>(),
                        "records": a.len()
                    }),
                );
                a
            }
            Err(e) => {
                audit.record_step(
                    "normalize",
                    step_start,
                    serde_json::json!({"passed": false}),
                );
                return Err(PipelineError {
                    app_error: e,
                    audit,
                    pre_violations: vec![],
                    post_violations: vec![],
                });
            }
        };

        // Stage 4: Post-transform boundary validation (centralized unified schema)
        let step_start = Instant::now();
        let mut post_violations = Vec::new();
        for assessment in &assessments {
            post_violations.extend(schema::validate_post_transform(assessment));
        }
        audit.record_step(
            "validate_post_transform",
            step_start,
            serde_json::json!({
                "violations": post_violations.len(),
                "passed": post_violations.is_empty()
            }),
        );

        if !post_violations.is_empty() {
            return Err(PipelineError {
                app_error: schema::violations_to_app_error(&post_violations),
                audit,
                pre_violations: vec![],
                post_violations,
            });
        }

        // Stage 5: Validate each output record (existing validation)
        let step_start = Instant::now();
        for assessment in &assessments {
            if let Err(e) = assessment.validate() {
                audit.record_step(
                    "validate_output",
                    step_start,
                    serde_json::json!({"passed": false}),
                );
                return Err(PipelineError {
                    app_error: e,
                    audit,
                    pre_violations: vec![],
                    post_violations: vec![],
                });
            }
        }
        audit.record_step(
            "validate_output",
            step_start,
            serde_json::json!({"passed": true}),
        );

        // Stage 6: PII scrubbing — apply patient_id strategy based on internal policy.
        // If internal PII is allowed and store_patient_id_plaintext is true, the pipeline
        // preserves the original patient_id for DB storage. The API response handler
        // applies scrubbing separately for external-facing output.
        let mut assessments = assessments;
        if let Some(pii_cfg) = pii_config {
            let step_start = Instant::now();
            let scrubbed = !pii_cfg.should_store_patient_id_plaintext();
            if scrubbed {
                for assessment in &mut assessments {
                    assessment.patient_id =
                        pii::scrub_patient_id(&assessment.patient_id, &pii_cfg.patient_id_strategy, &pii_cfg.hash_salt);
                }
            }
            audit.record_step(
                "pii_scrub",
                step_start,
                serde_json::json!({
                    "patient_id_scrubbed": scrubbed,
                    "strategy": if scrubbed { format!("{:?}", pii_cfg.patient_id_strategy) } else { "plaintext_internal".to_string() },
                }),
            );
        }

        info!(
            provider = provider.name(),
            count = assessments.len(),
            "pipeline completed"
        );

        Ok(PipelineResult {
            assessments,
            audit,
            pre_violations: vec![],
            post_violations: vec![],
        })
    }
}

/// Pipeline error that carries audit context and schema violations alongside the user-facing error.
pub struct PipelineError {
    pub app_error: AppError,
    pub audit: AuditBuilder,
    pub pre_violations: Vec<ViolationDetail>,
    pub post_violations: Vec<ViolationDetail>,
}

impl std::fmt::Debug for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PipelineError({})", self.app_error)
    }
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.app_error)
    }
}
