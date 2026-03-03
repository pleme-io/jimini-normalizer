use std::sync::Arc;

use dashmap::DashMap;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::model::unified::UnifiedAssessment;
use crate::provider::ProviderRegistry;

pub struct AppState {
    pub config: AppConfig,
    pub providers: ProviderRegistry,
    pub assessments: DashMap<Uuid, UnifiedAssessment>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            providers: ProviderRegistry::new(),
            assessments: DashMap::new(),
        })
    }
}
