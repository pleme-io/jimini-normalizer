use jimini_normalizer::config::SinkInstanceConfig;
use jimini_normalizer::sink::SinkRegistry;

fn http_sink_config(name: &str, endpoint: &str) -> SinkInstanceConfig {
    SinkInstanceConfig {
        name: name.to_string(),
        sink_type: "http_json".to_string(),
        endpoint: endpoint.to_string(),
        request_timeout_secs: 10,
        headers: std::collections::HashMap::new(),
    }
}

fn stdout_sink_config(name: &str) -> SinkInstanceConfig {
    SinkInstanceConfig {
        name: name.to_string(),
        sink_type: "stdout".to_string(),
        endpoint: String::new(),
        request_timeout_secs: 10,
        headers: std::collections::HashMap::new(),
    }
}

#[test]
fn test_registry_with_all_sinks() {
    let configs = vec![
        http_sink_config("vector", "http://localhost:8080"),
        stdout_sink_config("debug"),
    ];
    let registry = SinkRegistry::new(&configs);

    assert_eq!(registry.sink_names().len(), 2);
    assert!(registry.get("vector").is_some());
    assert!(registry.get("debug").is_some());
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_registry_with_filter() {
    let configs = vec![
        http_sink_config("vector", "http://localhost:8080"),
        stdout_sink_config("debug"),
    ];
    let enabled = vec!["vector".to_string()];
    let registry = SinkRegistry::with_filter(&configs, Some(&enabled));

    assert_eq!(registry.sink_names().len(), 1);
    assert!(registry.get("vector").is_some());
    assert!(registry.get("debug").is_none());
}

#[test]
fn test_registry_empty_filter_enables_all() {
    let configs = vec![
        http_sink_config("vector", "http://localhost:8080"),
        stdout_sink_config("debug"),
    ];
    let enabled: Vec<String> = vec![];
    let registry = SinkRegistry::with_filter(&configs, Some(&enabled));

    assert_eq!(registry.sink_names().len(), 2);
}

#[test]
fn test_registry_no_configs() {
    let configs: Vec<SinkInstanceConfig> = vec![];
    let registry = SinkRegistry::new(&configs);

    assert!(registry.is_empty());
    assert_eq!(registry.sink_names().len(), 0);
}

#[test]
fn test_registry_unknown_sink_type_skipped() {
    let configs = vec![SinkInstanceConfig {
        name: "unknown".to_string(),
        sink_type: "kafka".to_string(),
        endpoint: String::new(),
        request_timeout_secs: 10,
        headers: std::collections::HashMap::new(),
    }];
    let registry = SinkRegistry::new(&configs);

    assert!(registry.is_empty());
}

#[test]
fn test_sink_type_values() {
    let configs = vec![
        http_sink_config("vector", "http://localhost:8080"),
        stdout_sink_config("debug"),
    ];
    let registry = SinkRegistry::new(&configs);

    assert_eq!(registry.get("vector").unwrap().sink_type(), "http_json");
    assert_eq!(registry.get("debug").unwrap().sink_type(), "stdout");
}

#[test]
fn test_all_returns_all_sinks() {
    let configs = vec![
        http_sink_config("a", "http://a"),
        http_sink_config("b", "http://b"),
        stdout_sink_config("c"),
    ];
    let registry = SinkRegistry::new(&configs);

    assert_eq!(registry.all().len(), 3);
}

#[tokio::test]
async fn test_stdout_sink_send() {
    let config = stdout_sink_config("test-stdout");
    let sink = jimini_normalizer::sink::stdout::StdoutSink::new(&config);

    use jimini_normalizer::sink::Sink;
    let payload = serde_json::json!({"test": "value"});
    let result = sink.send(&payload).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_http_sink_no_endpoint_returns_error() {
    let config = http_sink_config("empty", "");
    let sink = jimini_normalizer::sink::http_json::HttpJsonSink::new(&config);

    use jimini_normalizer::sink::Sink;
    let payload = serde_json::json!({"test": "value"});
    let result = sink.send(&payload).await;
    assert!(result.is_err());
}
