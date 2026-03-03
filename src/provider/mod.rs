pub mod provider_a;
pub mod provider_b;
pub mod provider_c;

use std::collections::HashMap;

use crate::error::AppError;
use crate::model::unified::UnifiedAssessment;

/// Trait implemented by each provider adapter.
/// Encapsulates format-specific parsing, validation, and normalization.
pub trait NormalizationProvider: Send + Sync {
    fn name(&self) -> &str;
    fn format(&self) -> &str;
    fn validate_input(&self, raw: &[u8]) -> Result<(), AppError>;
    fn normalize(&self, raw: &[u8]) -> Result<Vec<UnifiedAssessment>, AppError>;
}

/// Registry of all known providers, keyed by name.
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn NormalizationProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            providers: HashMap::new(),
        };
        registry.register(Box::new(provider_a::ProviderA));
        registry.register(Box::new(provider_b::ProviderB));
        registry.register(Box::new(provider_c::ProviderC));
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
