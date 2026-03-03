# Testing Strategy

## Philosophy

Tests verify **behavior, not implementation**. Each test answers a specific question:
- Does this adapter correctly transform its source format?
- Does the pipeline reject invalid data at the right stage?
- Does the API return the correct HTTP semantics?

We avoid testing internal implementation details (private functions, struct layouts) and focus on the public contract: input → output.

## Test Structure

### Provider Adapter Tests (`tests/provider_adapter_test.rs`)

**Purpose:** Verify each adapter correctly parses, scales, and maps its source format to the unified schema.

**What they test:**
- Provider A: Nested JSON parsing, 0-10 → 0-100 score scaling, field mapping
- Provider B: Flat KV parsing, no-op score scaling (already 0-100), date parsing
- Provider C: CSV parsing, row grouping by patient/date/type, 0-10 → 0-100 scaling
- Malformed input rejection

**Why these are critical:** Adapters are the core business logic. If a score is scaled incorrectly or a date parsed wrong, every downstream consumer gets bad data. These tests use real fixture files to catch format regressions.

### Pipeline Tests (`tests/pipeline_test.rs`)

**Purpose:** Verify the staged pipeline correctly sequences validation → normalization → output validation, and that errors propagate with context.

**What they test:**
- Invalid input is rejected before normalization runs
- Out-of-range scores (e.g., 150 after scaling) fail output validation
- Valid input passes through all stages

**Why these are critical:** The pipeline enforces data quality invariants. If output validation is skipped or runs in the wrong order, invalid data reaches consumers silently.

### API Integration Tests (`tests/api_integration_test.rs`)

**Purpose:** Verify HTTP semantics end-to-end — status codes, response shapes, and data persistence across requests.

**What they test:**
- Health endpoint returns 200 with status/version
- Single normalization returns 200 with correct unified assessments
- Unknown provider returns 400
- Normalized assessments are retrievable by ID (round-trip)
- Batch endpoint returns 207 for partial failures with both successes and errors

**Why these are critical:** These are the tests a consumer would write against our API contract. They catch routing issues, serialization bugs, and state management problems that unit tests miss.

## What We Don't Test (and Why)

- **Framework behavior** — We don't test that Axum routes correctly or that serde deserializes JSON. These are well-tested upstream dependencies.
- **DashMap concurrency** — We don't test that concurrent inserts work. DashMap is battle-tested; our usage is straightforward.
- **Tracing output** — We don't assert on log format. Log format is an operational concern, not a correctness concern.

## Running Tests

```bash
# Run all tests
cargo test

# Run a specific test suite
cargo test --test provider_adapter_test
cargo test --test pipeline_test
cargo test --test api_integration_test

# Run with output
cargo test -- --nocapture
```

## Adding Tests for New Providers

When adding a new provider:
1. Add a fixture file in `fixtures/`
2. Add adapter tests in `provider_adapter_test.rs` (parsing, scaling, field mapping)
3. Add an integration test for the HTTP round-trip
4. No pipeline test changes needed — the pipeline is provider-agnostic
