use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

use crate::config::{BatchConfig, RetryConfig};

/// A single item queued for NATS emission.
pub struct EmitItem {
    pub subject: String,
    pub payload: Vec<u8>,
}

/// Result of a batch emit operation.
pub struct EmitReport {
    pub succeeded: usize,
    pub failed: usize,
    /// Items that failed all retry attempts — caller must handle these.
    pub dead_letters: Vec<EmitItem>,
}

/// Parallel batch emitter with configurable memory lifecycle.
///
/// Memory lifecycle strategy:
/// 1. Items pushed into internal Vec via `push()` — Rust owns the memory.
/// 2. `emit_all()` drains the buffer and publishes in parallel to NATS.
/// 3. After `cleanup_after_batches` emit cycles, internal Vecs are shrunk
///    via `shrink_to_fit()` to reclaim over-allocated capacity.
/// 4. Rust's ownership model guarantees all buffers are freed on drop — no
///    manual cleanup, no GC, no leaks.
/// 5. Dead-letter items (failed all retries) are returned to the caller for
///    handling (re-queue, persist locally, or log and discard).
pub struct BatchEmitter {
    buffer: Vec<EmitItem>,
    js: jetstream::Context,
    batch_config: BatchConfig,
    retry_config: RetryConfig,
    batches_emitted: AtomicU64,
}

impl BatchEmitter {
    pub fn new(
        js: jetstream::Context,
        batch_config: BatchConfig,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            buffer: Vec::with_capacity(batch_config.buffer_size),
            js,
            batch_config,
            retry_config,
            batches_emitted: AtomicU64::new(0),
        }
    }

    /// Push an item into the buffer. O(1) amortized.
    pub fn push(&mut self, item: EmitItem) {
        self.buffer.push(item);
    }

    /// Push multiple items at once.
    pub fn extend(&mut self, items: impl IntoIterator<Item = EmitItem>) {
        self.buffer.extend(items);
    }

    /// Number of items currently buffered.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Drain the buffer and emit all items to NATS in parallel.
    ///
    /// Uses a semaphore to cap concurrency at `max_concurrent_emits`.
    /// Each publish is retried with exponential backoff on failure.
    /// Returns an EmitReport with success/failure counts and dead letters.
    ///
    /// After emission, checks whether to reclaim memory based on the
    /// `cleanup_after_batches` threshold.
    pub async fn emit_all(&mut self) -> EmitReport {
        if self.buffer.is_empty() {
            return EmitReport {
                succeeded: 0,
                failed: 0,
                dead_letters: Vec::new(),
            };
        }

        // Drain buffer — ownership transfers out. Buffer is now empty and
        // retains its capacity for reuse (no reallocation on next batch).
        let items: Vec<EmitItem> = std::mem::take(&mut self.buffer);
        let total = items.len();

        let semaphore = Arc::new(Semaphore::new(self.batch_config.max_concurrent_emits));
        let js = self.js.clone();
        let retry_cfg = self.retry_config.clone();

        // Fan-out: publish all items in parallel with bounded concurrency
        let mut handles = Vec::with_capacity(total);

        for item in items {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let js = js.clone();
            let retry_cfg = retry_cfg.clone();

            let handle = tokio::spawn(async move {
                let result = publish_with_retry(&js, &item.subject, &item.payload, &retry_cfg).await;
                drop(permit); // release semaphore slot
                match result {
                    Ok(()) => Ok(()),
                    Err(_) => Err(item), // return dead letter
                }
            });
            handles.push(handle);
        }

        // Collect results
        let mut succeeded = 0usize;
        let mut dead_letters = Vec::new();

        for handle in handles {
            match handle.await {
                Ok(Ok(())) => succeeded += 1,
                Ok(Err(dead_item)) => dead_letters.push(dead_item),
                Err(join_err) => {
                    error!(error = %join_err, "emit task panicked");
                }
            }
        }

        let failed = dead_letters.len();
        let batch_num = self.batches_emitted.fetch_add(1, Ordering::Relaxed) + 1;

        debug!(
            total,
            succeeded,
            failed,
            batch_num,
            "batch emit complete"
        );

        // Memory lifecycle: reclaim over-allocated capacity after N batches
        if batch_num % self.batch_config.cleanup_after_batches as u64 == 0 {
            self.buffer.shrink_to_fit();
            info!(
                batch_num,
                "memory cleanup: buffer shrunk after {} batches",
                self.batch_config.cleanup_after_batches
            );
        }

        EmitReport {
            succeeded,
            failed,
            dead_letters,
        }
    }
}

/// Publish a single message to NATS JetStream with exponential backoff retry.
///
/// The caller retains ownership of the payload until NATS confirms receipt
/// (JetStream publish ack). On failure, the data stays in memory — never lost.
pub async fn publish_with_retry(
    js: &jetstream::Context,
    subject: &str,
    payload: &[u8],
    retry_cfg: &RetryConfig,
) -> Result<(), async_nats::Error> {
    let mut attempt = 0u32;

    loop {
        attempt += 1;

        match js
            .publish(subject.to_string(), payload.to_vec().into())
            .await
        {
            Ok(ack_future) => match ack_future.await {
                Ok(_) => return Ok(()),
                Err(e) if attempt < retry_cfg.max_attempts => {
                    let delay = backoff_delay(attempt, retry_cfg);
                    warn!(
                        attempt,
                        max = retry_cfg.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        subject,
                        "NATS publish ack failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    error!(
                        attempt,
                        error = %e,
                        subject,
                        "NATS publish failed after all retries"
                    );
                    return Err(e.into());
                }
            },
            Err(e) if attempt < retry_cfg.max_attempts => {
                let delay = backoff_delay(attempt, retry_cfg);
                warn!(
                    attempt,
                    max = retry_cfg.max_attempts,
                    delay_ms = delay.as_millis() as u64,
                    error = %e,
                    subject,
                    "NATS publish failed, retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => {
                error!(
                    attempt,
                    error = %e,
                    subject,
                    "NATS publish failed after all retries"
                );
                return Err(e.into());
            }
        }
    }
}

/// Calculate exponential backoff delay with jitter, capped at max_delay_ms.
/// Public alias for use in other modules (nats.rs outbox publish).
pub fn backoff_delay_pub(attempt: u32, cfg: &RetryConfig) -> Duration {
    backoff_delay(attempt, cfg)
}

/// Calculate exponential backoff delay with jitter, capped at max_delay_ms.
fn backoff_delay(attempt: u32, cfg: &RetryConfig) -> Duration {
    let base = cfg.base_delay_ms as u128;
    let exp = base.saturating_mul(2u128.saturating_pow(attempt.saturating_sub(1)));
    let capped = exp.min(cfg.max_delay_ms as u128) as u64;
    // Add ±25% jitter to avoid thundering herd
    let jitter = capped / 4;
    let actual = capped.saturating_sub(jitter / 2)
        + (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64
            % jitter.max(1));
    Duration::from_millis(actual)
}
