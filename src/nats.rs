use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::stream::RetentionPolicy;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::observability::{nats_dedup_total, nats_publish_total};

/// Connect to NATS with retry on initial connect.
pub async fn connect(url: &str) -> Result<async_nats::Client, async_nats::ConnectError> {
    let client = async_nats::ConnectOptions::new()
        .name("jimini-normalizer")
        .retry_on_initial_connect()
        .event_callback(|event| async move {
            match event {
                async_nats::Event::Disconnected => {
                    tracing::warn!("NATS disconnected");
                }
                async_nats::Event::Connected => {
                    tracing::info!("NATS connected");
                }
                async_nats::Event::ClientError(err) => {
                    tracing::error!(error = %err, "NATS client error");
                }
                _ => {}
            }
        })
        .connect(url)
        .await?;

    info!(url, "connected to NATS");
    Ok(client)
}

/// Create a JetStream context from a NATS client.
pub fn jetstream_context(client: &async_nats::Client) -> jetstream::Context {
    jetstream::new(client.clone())
}

/// Ensure both assessment and error streams exist (idempotent).
pub async fn ensure_streams(
    js: &jetstream::Context,
    config: &AppConfig,
) -> Result<(), async_nats::Error> {
    // Assessment stream — WorkQueue retention: message removed after consumer ack
    js.get_or_create_stream(jetstream::stream::Config {
        name: config.nats.stream_name.clone(),
        subjects: vec![format!(
            "{}.>",
            config.nats.stream_name.to_lowercase()
        )],
        retention: RetentionPolicy::WorkQueue,
        max_age: Duration::from_secs(7 * 24 * 3600), // 7 days
        ..Default::default()
    })
    .await?;

    info!(
        stream = config.nats.stream_name,
        "ensured assessment stream"
    );

    // Error stream — WorkQueue retention: errors processed once by worker
    js.get_or_create_stream(jetstream::stream::Config {
        name: config.nats.error_stream_name.clone(),
        subjects: vec![format!(
            "{}.>",
            config.nats.error_stream_name.to_lowercase()
        )],
        retention: RetentionPolicy::WorkQueue,
        max_age: Duration::from_secs(30 * 24 * 3600), // 30 days
        ..Default::default()
    })
    .await?;

    info!(
        stream = config.nats.error_stream_name,
        "ensured error stream"
    );

    // Outbox stream — WorkQueue retention: message removed after consumer ack.
    // duplicate_window prevents the same assessment from being published twice
    // (e.g. worker crash between DB commit and NATS ack, then sweeper re-publishes).
    // Window MUST be > sweeper_stale_after_secs (validated at startup).
    js.get_or_create_stream(jetstream::stream::Config {
        name: config.outbox.stream_name.clone(),
        subjects: vec![format!(
            "{}.>",
            config.outbox.stream_name.to_lowercase()
        )],
        retention: RetentionPolicy::WorkQueue,
        max_age: Duration::from_secs(7 * 24 * 3600), // 7 days
        duplicate_window: Duration::from_secs(config.outbox.duplicate_window_secs),
        ..Default::default()
    })
    .await?;

    info!(
        stream = config.outbox.stream_name,
        "ensured outbox stream"
    );

    Ok(())
}

/// Publish a JSON payload to the assessment stream with retry.
pub async fn publish_assessment(
    js: &jetstream::Context,
    stream_name: &str,
    provider: &str,
    payload: &serde_json::Value,
    retry_cfg: &crate::config::RetryConfig,
) -> Result<(), async_nats::Error> {
    let subject = format!("{}.{}", stream_name.to_lowercase(), provider);
    let data = serde_json::to_vec(payload).map_err(|e| {
        async_nats::Error::from(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    match crate::buffer::publish_with_retry(js, &subject, &data, retry_cfg).await {
        Ok(()) => {
            nats_publish_total()
                .with_label_values(&[stream_name, "success"])
                .inc();
            Ok(())
        }
        Err(e) => {
            nats_publish_total()
                .with_label_values(&[stream_name, "error"])
                .inc();
            Err(e)
        }
    }
}

/// Publish a payload to the outbox stream for sink delivery.
///
/// Uses `Nats-Msg-Id` header (assessment UUID) for server-side dedup within the
/// duplicate_window. This prevents duplicate outbox entries when:
/// - The worker crashes between DB commit and NATS ack, then the reconciliation
///   sweeper re-publishes the same assessment.
/// - NATS redelivery causes persist_assessment to re-run (DB is idempotent,
///   but the NATS outbox publish would create a duplicate without this header).
pub async fn publish_outbox(
    js: &jetstream::Context,
    outbox_stream_name: &str,
    assessment_id: &uuid::Uuid,
    payload: &serde_json::Value,
    retry_cfg: &crate::config::RetryConfig,
) -> Result<(), async_nats::Error> {
    let subject = format!("{}.delivery", outbox_stream_name.to_lowercase());
    let data = serde_json::to_vec(payload).map_err(|e| {
        async_nats::Error::from(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(
        "Nats-Msg-Id",
        assessment_id.to_string().as_str(),
    );

    let mut attempt = 0u32;
    loop {
        attempt += 1;

        let publish = js
            .publish_with_headers(
                subject.clone(),
                headers.clone(),
                data.clone().into(),
            )
            .await;

        match publish {
            Ok(ack_future) => match ack_future.await {
                Ok(ack) => {
                    if ack.duplicate {
                        nats_dedup_total().inc();
                        tracing::debug!(
                            %assessment_id,
                            "outbox publish dedup: message already in stream"
                        );
                    }
                    nats_publish_total()
                        .with_label_values(&[outbox_stream_name, "success"])
                        .inc();
                    return Ok(());
                }
                Err(e) if attempt < retry_cfg.max_attempts => {
                    let delay = crate::buffer::backoff_delay_pub(attempt, retry_cfg);
                    warn!(
                        attempt,
                        max = retry_cfg.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        %assessment_id,
                        "outbox publish ack failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    nats_publish_total()
                        .with_label_values(&[outbox_stream_name, "error"])
                        .inc();
                    return Err(e.into());
                }
            },
            Err(e) if attempt < retry_cfg.max_attempts => {
                let delay = crate::buffer::backoff_delay_pub(attempt, retry_cfg);
                warn!(
                    attempt,
                    max = retry_cfg.max_attempts,
                    delay_ms = delay.as_millis() as u64,
                    error = %e,
                    %assessment_id,
                    "outbox publish failed, retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => {
                nats_publish_total()
                    .with_label_values(&[outbox_stream_name, "error"])
                    .inc();
                return Err(e.into());
            }
        }
    }
}

/// Publish an error event to the error stream with retry.
pub async fn publish_error(
    js: &jetstream::Context,
    error_stream_name: &str,
    provider: &str,
    error_payload: &serde_json::Value,
    retry_cfg: &crate::config::RetryConfig,
) -> Result<(), async_nats::Error> {
    let subject = format!("{}.{}", error_stream_name.to_lowercase(), provider);
    let data = serde_json::to_vec(error_payload).map_err(|e| {
        async_nats::Error::from(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    match crate::buffer::publish_with_retry(js, &subject, &data, retry_cfg).await {
        Ok(()) => {
            nats_publish_total()
                .with_label_values(&[error_stream_name, "success"])
                .inc();
            Ok(())
        }
        Err(e) => {
            nats_publish_total()
                .with_label_values(&[error_stream_name, "error"])
                .inc();
            Err(e)
        }
    }
}
