pub mod provider_a;
pub mod provider_b;
pub mod provider_c;
pub mod schema;

use std::collections::HashMap;

use tracing::info;

use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;

/// Trait implemented by each provider adapter.
///
/// To add a new provider:
/// 1. Create `src/provider/provider_x.rs` implementing this trait
/// 2. Add `pub mod provider_x;` to this file
/// 3. Add to the `all_providers()` function below
/// 4. Add PII field rules in the config.yaml under `pii.provider_fields.provider_x`
/// 5. Add test fixtures and adapter tests
///
/// The provider trait is intentionally simple — each provider encapsulates
/// all format-specific parsing, validation, and normalization. The pipeline
/// orchestrates the lifecycle (validation → normalize → PII scrub → audit).
pub trait NormalizationProvider: Send + Sync {
    /// Unique name for this provider (used as lookup key and in audit trail).
    fn name(&self) -> &str;

    /// Human-readable format description (e.g. "nested_json", "flat_kv", "csv").
    fn format(&self) -> &str;

    /// Validate raw input without normalizing. Called before normalize() to
    /// catch structural issues early. Return Ok(()) if input is structurally valid.
    fn validate_input(&self, raw: &[u8]) -> Result<(), AppError>;

    /// Parse and normalize raw input into unified assessments. Handles
    /// format-specific parsing, score scaling (to 0-100), and field mapping.
    fn normalize(&self, raw: &[u8]) -> Result<Vec<UnifiedAssessment>, AppError>;
}

/// Returns all available provider implementations.
///
/// This is the single registration point. Adding a new provider requires
/// adding it to this list. The function is called once at startup.
fn all_providers() -> Vec<Box<dyn NormalizationProvider>> {
    vec![
        Box::new(provider_a::ProviderA),
        Box::new(provider_b::ProviderB),
        Box::new(provider_c::ProviderC),
    ]
}

/// Registry of all known providers, keyed by name.
///
/// Optionally filters to only the providers listed in `enabled_providers`.
/// If `enabled_providers` is None or empty, all providers are registered.
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn NormalizationProvider>>,
}

impl ProviderRegistry {
    /// Create a registry with all providers enabled.
    pub fn new() -> Self {
        Self::with_filter(None)
    }

    /// Create a registry filtered to only the specified providers.
    /// If `enabled` is None or empty, all providers are registered.
    pub fn with_filter(enabled: Option<&[String]>) -> Self {
        let mut registry = Self {
            providers: HashMap::new(),
        };

        for provider in all_providers() {
            let name = provider.name().to_string();
            if let Some(enabled_list) = enabled {
                if !enabled_list.is_empty() && !enabled_list.iter().any(|e| e == &name) {
                    info!(provider = %name, "provider available but not enabled — skipping");
                    continue;
                }
            }
            info!(provider = %name, format = %provider.format(), "registered provider");
            registry.register(provider);
        }

        registry
    }

    fn register(&mut self, provider: Box<dyn NormalizationProvider>) {
        self.providers
            .insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Result<&dyn NormalizationProvider, AppError> {
        self.providers
            .get(name)
            .map(|p| p.as_ref())
            .ok_or_else(|| AppError::UnknownProvider(name.to_string()))
    }

    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}
