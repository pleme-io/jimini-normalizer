use std::collections::HashMap;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// PII/PHI scrubbing configuration.
///
/// **MANDATORY** — the service will refuse to start if this section is missing
/// or empty. There are NO defaults. Every deployment must explicitly declare
/// which fields contain PII and how they should be handled.
///
/// This ensures HIPAA/SOC2 compliance is auditable: the config file IS the
/// declaration of what data is considered sensitive and how it's protected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiConfig {
    /// If false, the service will panic on startup. Must be explicitly set to true.
    pub enabled: bool,

    /// How to handle patient_id in **external-facing** contexts (API responses,
    /// logs, metrics labels). Does NOT affect internal DB storage when
    /// `internal.store_patient_id_plaintext` is true.
    pub patient_id_strategy: ScrubStrategy,

    /// Per-provider field scrubbing rules. Key is provider name (e.g. "provider_a").
    /// Every provider with PHI fields MUST have rules declared here.
    pub provider_fields: HashMap<String, Vec<FieldRule>>,

    /// Strategy for raw input in error/failure paths (NATS error stream, failed_records).
    pub raw_input_strategy: ScrubStrategy,

    /// HMAC-SHA256 key for salted hashing. **Required** when using `Hash` strategy.
    /// Injected from the `PII_HASH_SALT` environment variable (K8s secret).
    /// Must be at least 32 characters. Different per environment to prevent
    /// cross-environment correlation and rainbow table attacks.
    #[serde(default)]
    pub hash_salt: String,

    /// Internal subsystem PII policies — controls what PII flows between
    /// internal services (DB, NATS, worker). These must be explicitly configured.
    pub internal: InternalPiiPolicy,
}

/// Controls PII flow within the trusted internal boundary (DB, NATS, worker).
///
/// When `allow_internal_pii` is true, the service stores PII in internal systems
/// (encrypted at rest via Postgres TDE / SOPS secrets) while still scrubbing
/// PII in all external-facing outputs (API responses, logs, metrics).
///
/// This enables features like failed record replay while maintaining external compliance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalPiiPolicy {
    /// Allow PII to flow between internal subsystems (DB, NATS, worker).
    /// When true: patient_id stored plaintext in DB, raw input stored for replay.
    /// When false: all PII is scrubbed before any persistence.
    /// Must be explicitly set — no default.
    pub allow_internal_pii: bool,

    /// Store patient_id in plaintext in the assessments table.
    /// Requires `allow_internal_pii: true`. Enables patient lookup queries.
    /// The API response still applies `patient_id_strategy` scrubbing.
    pub store_patient_id_plaintext: bool,

    /// Store raw input in failed_records and schema_violations for replay.
    /// Requires `allow_internal_pii: true`.
    /// When false, only a hash/redacted marker is stored.
    pub store_raw_for_replay: bool,
}

/// How to scrub a sensitive field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScrubStrategy {
    /// Replace with HMAC-SHA256 salted hash (one-way, deterministic within the
    /// same environment — allows correlation without exposure. Salted to prevent
    /// rainbow table attacks.)
    Hash,
    /// Replace with "[REDACTED]"
    Redact,
}

/// A per-provider field scrubbing rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldRule {
    /// Field name in the provider's input payload (e.g. "name", "dob", "patient_name")
    pub field: String,
    /// How to scrub this field
    pub strategy: ScrubStrategy,
}

impl PiiConfig {
    /// Validate the PII config. Panics with a clear message if invalid.
    /// Called at startup — service MUST NOT run with bad PII config.
    pub fn validate_or_panic(&self) {
        assert!(
            self.enabled,
            "FATAL: pii.enabled must be explicitly set to true. \
             The service cannot run without PII scrubbing configured. \
             Set pii.enabled: true in your config.yaml and declare all PII field rules."
        );

        // Hash strategy requires a salt
        let uses_hash = self.patient_id_strategy == ScrubStrategy::Hash
            || self.raw_input_strategy == ScrubStrategy::Hash;
        if uses_hash {
            assert!(
                self.hash_salt.len() >= 32,
                "FATAL: pii.hash_salt must be at least 32 characters when using Hash strategy. \
                 Set PII_HASH_SALT environment variable with a cryptographically random key. \
                 Different environments MUST use different salts."
            );
        }

        // If internal PII is disallowed, storage flags must be false
        if !self.internal.allow_internal_pii {
            assert!(
                !self.internal.store_patient_id_plaintext,
                "FATAL: pii.internal.store_patient_id_plaintext requires \
                 pii.internal.allow_internal_pii to be true."
            );
            assert!(
                !self.internal.store_raw_for_replay,
                "FATAL: pii.internal.store_raw_for_replay requires \
                 pii.internal.allow_internal_pii to be true."
            );
        }
    }

    /// Whether raw input should be stored (for replay) vs scrubbed.
    pub fn should_store_raw_input(&self) -> bool {
        self.internal.allow_internal_pii && self.internal.store_raw_for_replay
    }

    /// Whether patient_id should be stored in plaintext in the DB.
    pub fn should_store_patient_id_plaintext(&self) -> bool {
        self.internal.allow_internal_pii && self.internal.store_patient_id_plaintext
    }
}

/// Apply the patient_id scrubbing strategy (for external-facing contexts).
pub fn scrub_patient_id(patient_id: &str, strategy: &ScrubStrategy, salt: &str) -> String {
    match strategy {
        ScrubStrategy::Hash => hmac_sha256_hex(patient_id.as_bytes(), salt),
        ScrubStrategy::Redact => "[REDACTED]".to_string(),
    }
}

/// Apply raw input scrubbing strategy — returns either an HMAC hash or "[REDACTED]".
pub fn scrub_raw_input(raw: &[u8], strategy: &ScrubStrategy, salt: &str) -> String {
    match strategy {
        ScrubStrategy::Hash => hmac_sha256_hex(raw, salt),
        ScrubStrategy::Redact => "[REDACTED]".to_string(),
    }
}

/// Conditionally scrub or preserve raw input based on PII policy.
pub fn process_raw_input(raw: &[u8], pii_config: &PiiConfig) -> String {
    if pii_config.should_store_raw_input() {
        String::from_utf8_lossy(raw).to_string()
    } else {
        scrub_raw_input(raw, &pii_config.raw_input_strategy, &pii_config.hash_salt)
    }
}

/// Conditionally scrub or preserve patient_id for internal storage.
pub fn process_patient_id_for_storage(patient_id: &str, pii_config: &PiiConfig) -> String {
    if pii_config.should_store_patient_id_plaintext() {
        patient_id.to_string()
    } else {
        scrub_patient_id(patient_id, &pii_config.patient_id_strategy, &pii_config.hash_salt)
    }
}

/// HMAC-SHA256 hex hash of arbitrary bytes using the environment-specific salt.
///
/// HMAC prevents rainbow table attacks — identical inputs in different environments
/// produce different hashes because each environment has a unique salt.
fn hmac_sha256_hex(input: &[u8], salt: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(salt.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(input);
    let result = mac.finalize();
    format!("sha256:{:x}", result.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SALT: &str = "test-salt-at-least-32-characters-long-for-tests";

    fn make_config(
        allow_internal: bool,
        store_patient: bool,
        store_raw: bool,
    ) -> PiiConfig {
        PiiConfig {
            enabled: true,
            patient_id_strategy: ScrubStrategy::Hash,
            provider_fields: HashMap::new(),
            raw_input_strategy: ScrubStrategy::Hash,
            hash_salt: TEST_SALT.to_string(),
            internal: InternalPiiPolicy {
                allow_internal_pii: allow_internal,
                store_patient_id_plaintext: store_patient,
                store_raw_for_replay: store_raw,
            },
        }
    }

    #[test]
    fn scrub_patient_id_hash_is_deterministic() {
        let a = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
        let b = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        assert!(!a.contains("P123"));
    }

    #[test]
    fn scrub_patient_id_different_salts_produce_different_hashes() {
        let a = scrub_patient_id("P123", &ScrubStrategy::Hash, "salt-a-at-least-32-characters-long");
        let b = scrub_patient_id("P123", &ScrubStrategy::Hash, "salt-b-at-least-32-characters-long");
        assert_ne!(a, b, "different salts must produce different hashes");
    }

    #[test]
    fn scrub_patient_id_redact() {
        let result = scrub_patient_id("P123", &ScrubStrategy::Redact, TEST_SALT);
        assert_eq!(result, "[REDACTED]");
    }

    #[test]
    fn scrub_raw_input_hash_does_not_contain_input() {
        let raw = b"sensitive patient data";
        let result = scrub_raw_input(raw, &ScrubStrategy::Hash, TEST_SALT);
        assert!(result.starts_with("sha256:"));
        assert!(!result.contains("sensitive"));
    }

    #[test]
    fn scrub_raw_input_redact() {
        let raw = b"sensitive patient data";
        let result = scrub_raw_input(raw, &ScrubStrategy::Redact, TEST_SALT);
        assert_eq!(result, "[REDACTED]");
    }

    #[test]
    #[should_panic(expected = "pii.enabled must be explicitly set to true")]
    fn validate_panics_when_disabled() {
        let mut cfg = make_config(false, false, false);
        cfg.enabled = false;
        cfg.validate_or_panic();
    }

    #[test]
    fn validate_passes_when_enabled() {
        let config = make_config(false, false, false);
        config.validate_or_panic();
    }

    #[test]
    #[should_panic(expected = "pii.hash_salt must be at least 32 characters")]
    fn validate_panics_when_hash_salt_too_short() {
        let mut cfg = make_config(false, false, false);
        cfg.hash_salt = "too-short".to_string();
        cfg.validate_or_panic();
    }

    #[test]
    #[should_panic(expected = "store_patient_id_plaintext requires")]
    fn validate_panics_on_inconsistent_patient_id_config() {
        let config = make_config(false, true, false);
        config.validate_or_panic();
    }

    #[test]
    #[should_panic(expected = "store_raw_for_replay requires")]
    fn validate_panics_on_inconsistent_raw_config() {
        let config = make_config(false, false, true);
        config.validate_or_panic();
    }

    #[test]
    fn internal_pii_allows_plaintext_storage() {
        let config = make_config(true, true, true);
        config.validate_or_panic();
        assert!(config.should_store_patient_id_plaintext());
        assert!(config.should_store_raw_input());
    }

    #[test]
    fn process_raw_input_stores_plaintext_when_allowed() {
        let config = make_config(true, true, true);
        let raw = b"sensitive data";
        let result = process_raw_input(raw, &config);
        assert_eq!(result, "sensitive data");
    }

    #[test]
    fn process_raw_input_scrubs_when_disallowed() {
        let config = make_config(true, true, false);
        let raw = b"sensitive data";
        let result = process_raw_input(raw, &config);
        assert!(result.starts_with("sha256:"));
        assert!(!result.contains("sensitive"));
    }

    #[test]
    fn process_patient_id_preserves_when_allowed() {
        let config = make_config(true, true, true);
        let result = process_patient_id_for_storage("P123", &config);
        assert_eq!(result, "P123");
    }

    #[test]
    fn process_patient_id_scrubs_when_disallowed() {
        let config = make_config(true, false, false);
        let result = process_patient_id_for_storage("P123", &config);
        assert!(result.starts_with("sha256:"));
    }

    #[test]
    fn hmac_hash_is_not_plain_sha256() {
        // Verify HMAC produces different output than plain SHA-256
        use sha2::Digest;
        let plain_hash = sha2::Sha256::digest(b"P123");
        let plain = format!("sha256:{plain_hash:x}");
        let hmac_result = scrub_patient_id("P123", &ScrubStrategy::Hash, TEST_SALT);
        assert_ne!(plain, hmac_result, "HMAC hash must differ from plain SHA-256");
    }
}
