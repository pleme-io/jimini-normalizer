use std::collections::HashMap;

use jimini_normalizer::config::AppConfig;

#[test]
fn test_override_enabled_providers() {
    let base = AppConfig::default();
    assert!(base.enabled_providers.is_empty());

    let mut overrides = HashMap::new();
    overrides.insert(
        "enabled_providers".to_string(),
        serde_json::json!(["provider_a", "provider_b"]),
    );

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.enabled_providers, vec!["provider_a", "provider_b"]);
}

#[test]
fn test_override_retry_config() {
    let base = AppConfig::default();
    assert_eq!(base.retry.max_attempts, 3);
    assert_eq!(base.retry.base_delay_ms, 100);
    assert_eq!(base.retry.max_delay_ms, 5000);

    let mut overrides = HashMap::new();
    overrides.insert("retry.max_attempts".to_string(), serde_json::json!(5));
    overrides.insert("retry.base_delay_ms".to_string(), serde_json::json!(200));
    overrides.insert("retry.max_delay_ms".to_string(), serde_json::json!(10000));

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.retry.max_attempts, 5);
    assert_eq!(merged.retry.base_delay_ms, 200);
    assert_eq!(merged.retry.max_delay_ms, 10000);
}

#[test]
fn test_override_batch_config() {
    let base = AppConfig::default();

    let mut overrides = HashMap::new();
    overrides.insert("batch.buffer_size".to_string(), serde_json::json!(200));
    overrides.insert("batch.max_concurrent_emits".to_string(), serde_json::json!(100));

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.batch.buffer_size, 200);
    assert_eq!(merged.batch.max_concurrent_emits, 100);
}

#[test]
fn test_override_retention_days() {
    let base = AppConfig::default();
    assert_eq!(base.retention.days, 90);

    let mut overrides = HashMap::new();
    overrides.insert("retention.days".to_string(), serde_json::json!(30));

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.retention.days, 30);
}

#[test]
fn test_override_refresh_interval() {
    let base = AppConfig::default();
    assert_eq!(base.dynamic_config.refresh_interval_secs, 30);

    let mut overrides = HashMap::new();
    overrides.insert(
        "dynamic_config.refresh_interval_secs".to_string(),
        serde_json::json!(10),
    );

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.dynamic_config.refresh_interval_secs, 10);
}

#[test]
fn test_unknown_keys_are_ignored() {
    let base = AppConfig::default();

    let mut overrides = HashMap::new();
    overrides.insert("unknown.key".to_string(), serde_json::json!("value"));
    overrides.insert("server.host".to_string(), serde_json::json!("evil.com"));
    overrides.insert("database.url".to_string(), serde_json::json!("postgres://evil"));
    overrides.insert("nats.url".to_string(), serde_json::json!("nats://evil"));
    overrides.insert("pii.enabled".to_string(), serde_json::json!(false));

    let merged = base.with_dynamic_overrides(&overrides);

    // None of these should have changed
    assert_eq!(merged.server.host, base.server.host);
    assert_eq!(merged.database.url, base.database.url);
    assert_eq!(merged.nats.url, base.nats.url);
}

#[test]
fn test_invalid_types_are_ignored() {
    let base = AppConfig::default();

    let mut overrides = HashMap::new();
    // retry.max_attempts expects u64, give it a string
    overrides.insert("retry.max_attempts".to_string(), serde_json::json!("not a number"));
    // enabled_providers expects string[], give it a number
    overrides.insert("enabled_providers".to_string(), serde_json::json!(42));

    let merged = base.with_dynamic_overrides(&overrides);

    // Should remain unchanged
    assert_eq!(merged.retry.max_attempts, base.retry.max_attempts);
    assert_eq!(merged.enabled_providers, base.enabled_providers);
}

#[test]
fn test_preserves_unset_fields() {
    let mut base = AppConfig::default();
    base.retry.max_attempts = 10;
    base.batch.buffer_size = 500;

    let mut overrides = HashMap::new();
    // Only override retry, leave batch alone
    overrides.insert("retry.max_attempts".to_string(), serde_json::json!(7));

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.retry.max_attempts, 7);
    assert_eq!(merged.batch.buffer_size, 500); // preserved from base
}

#[test]
fn test_override_outbox_enabled_sinks() {
    let base = AppConfig::default();
    assert!(base.outbox.enabled_sinks.is_empty());

    let mut overrides = HashMap::new();
    overrides.insert(
        "outbox.enabled_sinks".to_string(),
        serde_json::json!(["vector", "debug"]),
    );

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.outbox.enabled_sinks, vec!["vector", "debug"]);
}

#[test]
fn test_outbox_enabled_sinks_invalid_type_ignored() {
    let base = AppConfig::default();

    let mut overrides = HashMap::new();
    overrides.insert("outbox.enabled_sinks".to_string(), serde_json::json!(42));

    let merged = base.with_dynamic_overrides(&overrides);
    assert!(merged.outbox.enabled_sinks.is_empty());
}

#[test]
fn test_empty_overrides_returns_clone() {
    let base = AppConfig::default();
    let overrides = HashMap::new();

    let merged = base.with_dynamic_overrides(&overrides);
    assert_eq!(merged.retry.max_attempts, base.retry.max_attempts);
    assert_eq!(merged.enabled_providers, base.enabled_providers);
    assert_eq!(merged.retention.days, base.retention.days);
}
