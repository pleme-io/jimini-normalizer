use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::hash_input;
use crate::entity::schema_violation;
use crate::error::{AppError, FieldError};
use crate::model::unified::UnifiedAssessment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViolationDetail {
    pub field: String,
    pub expected: String,
    pub actual: String,
    pub message: String,
}

/// Validates input against provider-specific schema before transformation.
/// Returns violations (does not short-circuit the caller — the caller decides
/// whether to persist and continue or abort).
pub fn validate_pre_transform(
    provider_name: &str,
    raw: &[u8],
) -> Vec<ViolationDetail> {
    let mut violations = Vec::new();

    if raw.is_empty() {
        violations.push(ViolationDetail {
            field: "body".into(),
            expected: "non-empty input".into(),
            actual: "empty".into(),
            message: "input body is empty".into(),
        });
        return violations;
    }

    match provider_name {
        "provider_a" => validate_provider_a_schema(raw, &mut violations),
        "provider_b" => validate_provider_b_schema(raw, &mut violations),
        "provider_c" => validate_provider_c_schema(raw, &mut violations),
        _ => {
            violations.push(ViolationDetail {
                field: "provider".into(),
                expected: "known provider".into(),
                actual: provider_name.into(),
                message: format!("unknown provider: {provider_name}"),
            });
        }
    }

    violations
}

fn validate_provider_a_schema(raw: &[u8], violations: &mut Vec<ViolationDetail>) {
    let val: serde_json::Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => {
            violations.push(ViolationDetail {
                field: "body".into(),
                expected: "valid JSON".into(),
                actual: "invalid JSON".into(),
                message: format!("JSON parse error: {e}"),
            });
            return;
        }
    };

    if val.get("patient").is_none() {
        violations.push(ViolationDetail {
            field: "patient".into(),
            expected: "object".into(),
            actual: "missing".into(),
            message: "provider_a requires 'patient' object".into(),
        });
    } else if val["patient"].get("id").and_then(|v| v.as_str()).is_none() {
        violations.push(ViolationDetail {
            field: "patient.id".into(),
            expected: "string".into(),
            actual: format!("{}", val["patient"].get("id").unwrap_or(&serde_json::Value::Null)),
            message: "patient.id is required and must be a string".into(),
        });
    }

    if val.get("assessment").is_none() {
        violations.push(ViolationDetail {
            field: "assessment".into(),
            expected: "object".into(),
            actual: "missing".into(),
            message: "provider_a requires 'assessment' object".into(),
        });
    } else {
        if val["assessment"].get("type").and_then(|v| v.as_str()).is_none() {
            violations.push(ViolationDetail {
                field: "assessment.type".into(),
                expected: "string".into(),
                actual: "missing or non-string".into(),
                message: "assessment.type is required".into(),
            });
        }
        if val["assessment"].get("scores").and_then(|v| v.as_object()).is_none() {
            violations.push(ViolationDetail {
                field: "assessment.scores".into(),
                expected: "object".into(),
                actual: "missing or non-object".into(),
                message: "assessment.scores is required and must be an object".into(),
            });
        }
    }
}

fn validate_provider_b_schema(raw: &[u8], violations: &mut Vec<ViolationDetail>) {
    let val: serde_json::Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => {
            violations.push(ViolationDetail {
                field: "body".into(),
                expected: "valid JSON".into(),
                actual: "invalid JSON".into(),
                message: format!("JSON parse error: {e}"),
            });
            return;
        }
    };

    if val.get("patient_id").and_then(|v| v.as_str()).is_none() {
        violations.push(ViolationDetail {
            field: "patient_id".into(),
            expected: "string".into(),
            actual: "missing".into(),
            message: "patient_id is required".into(),
        });
    }

    if val.get("assessment_type").and_then(|v| v.as_str()).is_none() {
        violations.push(ViolationDetail {
            field: "assessment_type".into(),
            expected: "string".into(),
            actual: "missing".into(),
            message: "assessment_type is required".into(),
        });
    }

    // Check for at least one score_* key
    let has_scores = val
        .as_object()
        .map(|m| m.keys().any(|k| k.starts_with("score_")))
        .unwrap_or(false);
    if !has_scores {
        violations.push(ViolationDetail {
            field: "score_*".into(),
            expected: "at least one score_* key".into(),
            actual: "none found".into(),
            message: "provider_b requires at least one score_* field".into(),
        });
    }
}

fn validate_provider_c_schema(raw: &[u8], violations: &mut Vec<ViolationDetail>) {
    let input = match std::str::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => {
            violations.push(ViolationDetail {
                field: "body".into(),
                expected: "valid UTF-8 CSV".into(),
                actual: "invalid UTF-8".into(),
                message: "CSV input must be valid UTF-8".into(),
            });
            return;
        }
    };

    let mut rdr = csv::Reader::from_reader(input.as_bytes());
    let headers = rdr.headers();
    match headers {
        Ok(h) => {
            let expected = ["patient_id", "assessment_date", "metric_name", "metric_value", "category"];
            for col in &expected {
                if h.iter().all(|hdr| hdr != *col) {
                    violations.push(ViolationDetail {
                        field: col.to_string(),
                        expected: "column present in CSV header".into(),
                        actual: "missing".into(),
                        message: format!("CSV is missing required column: {col}"),
                    });
                }
            }
        }
        Err(e) => {
            violations.push(ViolationDetail {
                field: "headers".into(),
                expected: "valid CSV headers".into(),
                actual: "parse error".into(),
                message: format!("CSV header parse error: {e}"),
            });
        }
    }
}

/// Validates the unified output schema after transformation.
/// Catches any structural issues the pipeline might introduce.
pub fn validate_post_transform(assessment: &UnifiedAssessment) -> Vec<ViolationDetail> {
    let mut violations = Vec::new();

    if assessment.patient_id.is_empty() {
        violations.push(ViolationDetail {
            field: "patientId".into(),
            expected: "non-empty string".into(),
            actual: "empty".into(),
            message: "patientId must not be empty after normalization".into(),
        });
    }

    if assessment.assessment_type.is_empty() {
        violations.push(ViolationDetail {
            field: "assessmentType".into(),
            expected: "non-empty string".into(),
            actual: "empty".into(),
            message: "assessmentType must not be empty after normalization".into(),
        });
    }

    if assessment.scores.is_empty() {
        violations.push(ViolationDetail {
            field: "scores".into(),
            expected: "at least one score".into(),
            actual: "empty".into(),
            message: "scores array must not be empty after normalization".into(),
        });
    }

    for (i, score) in assessment.scores.iter().enumerate() {
        if !(0.0..=100.0).contains(&score.value) {
            violations.push(ViolationDetail {
                field: format!("scores[{i}].value"),
                expected: "0.0-100.0".into(),
                actual: format!("{}", score.value),
                message: format!("score value {} out of 0-100 range", score.value),
            });
        }
        if score.dimension.is_empty() {
            violations.push(ViolationDetail {
                field: format!("scores[{i}].dimension"),
                expected: "non-empty string".into(),
                actual: "empty".into(),
                message: "score dimension must not be empty".into(),
            });
        }
        if score.scale != "0-100" {
            violations.push(ViolationDetail {
                field: format!("scores[{i}].scale"),
                expected: "0-100".into(),
                actual: score.scale.clone(),
                message: "all scores must be on 0-100 scale after normalization".into(),
            });
        }
    }

    if assessment.metadata.source_provider.is_empty() {
        violations.push(ViolationDetail {
            field: "metadata.sourceProvider".into(),
            expected: "non-empty string".into(),
            actual: "empty".into(),
            message: "source provider must be recorded".into(),
        });
    }

    violations
}

/// Persist schema violations to the database for auditing.
///
/// **HIPAA/SOC2 compliant**: raw input is scrubbed per PII config.
pub async fn persist_violations(
    db: &DatabaseConnection,
    boundary: &str,
    provider: &str,
    raw_input: &[u8],
    violations: &[ViolationDetail],
    pii_config: &crate::pii::PiiConfig,
) -> Result<(), sea_orm::DbErr> {
    let model = schema_violation::ActiveModel {
        id: Set(Uuid::new_v4()),
        boundary: Set(boundary.to_string()),
        provider: Set(provider.to_string()),
        input_hash: Set(hash_input(raw_input)),
        violations: Set(serde_json::to_value(violations).unwrap_or_default()),
        raw_input: Set(crate::pii::process_raw_input(raw_input, pii_config)),
        created_at: Set(Utc::now().into()),
    };
    model.insert(db).await?;
    Ok(())
}

/// Convert schema violations to AppError::Validation for API response.
pub fn violations_to_app_error(violations: &[ViolationDetail]) -> AppError {
    AppError::Validation(
        violations
            .iter()
            .map(|v| FieldError {
                field: v.field.clone(),
                message: v.message.clone(),
            })
            .collect(),
    )
}
