# Testing Strategy

## What I Tested

### Provider Adapter Tests (`tests/provider_adapter_test.rs`) — 5 tests

Each adapter's core transformation logic is tested against real fixture data:

- **Provider A (nested JSON):** Parses nested `patient.id`, `assessment.scores{}` structure. Verifies 0-10 → 0-100 score scaling (anxiety 7 → 70, social 4 → 40). Confirms `source_format = "nested_json"` and correct field mapping.
- **Provider B (flat KV):** Extracts dimensions from `score_*` prefixed keys. Verifies scores pass through without scaling (already 0-100). Confirms PHI fields (`patient_name`, `notes`) are excluded from output.
- **Provider C (CSV):** Parses CSV rows, groups by `(patient_id, assessment_date, category)` into a single assessment. Verifies 0-10 → 0-100 metric scaling.
- **Input validation:** Provider A rejects malformed JSON. Provider B rejects payloads with no `score_*` keys.

### Pipeline Tests (`tests/pipeline_test.rs`) — 5 tests

The staged normalization pipeline is tested for correctness and data integrity:

- **Invalid input rejection:** Non-JSON input fails at the parse stage with a "parse error".
- **Output validation:** Scores outside 0-100 range after scaling (e.g., input 15 on 0-10 scale → 150) are caught by output validation.
- **Happy path:** Valid Provider B input passes all pipeline stages with scores in range.
- **camelCase serialization:** Output JSON uses `patientId`, `assessmentDate`, `sourceProvider` — never snake_case.
- **PHI non-propagation:** Patient names and dates of birth from input fixtures never appear in normalized output.

### API Integration Tests (`tests/api_integration_test.rs`) — 8 tests

Full HTTP round-trips via `axum-test`:

- **Health:** `GET /health` returns `{"status":"ok"}`.
- **Normalize all 3 providers:** Each returns 200 with correct unified schema shape, correct `patientId`, `assessmentType`, score counts, and metadata.
- **Unknown provider:** Returns 400.
- **Round-trip persistence:** Normalize → `GET /assessments/:id` retrieves the same assessment by UUID.
- **Batch partial failure:** `POST /normalize/batch` with one valid and one unknown provider returns 207 Multi-Status with `successes[1]` and `errors[1]`.
- **Metrics:** `GET /metrics` returns Prometheus format with `jimini_http_requests_total` and `jimini_normalizations_total`.

### Compose Integration Tests (`tests/compose_integration_test.sh`) — 48 assertions

End-to-end smoke tests against the Docker Compose service:

- All 3 providers normalize correctly with score scaling verification
- PHI fields never leak into responses (name, DOB)
- camelCase output format verification
- Error handling: unknown provider (400), malformed input (422)
- Assessment CRUD: normalize → retrieve by ID → list all
- Batch endpoint: 207 Multi-Status with mixed success/failure
- Prometheus metrics endpoint serves expected counters

```bash
# Run against already-running service
./tests/compose_integration_test.sh

# Build, start, test, and tear down automatically
COMPOSE_UP=1 ./tests/compose_integration_test.sh
```

## What I'd Test With More Time

- **Concurrency stress tests** — Parallel normalize requests to verify DashMap thread safety under load and check for race conditions in the in-memory store.
- **Large batch processing** — Batches with 100+ records to verify memory behavior, response time degradation, and partial failure reporting at scale.
- **Fuzz testing** — Property-based tests (proptest/quickcheck) generating random JSON/CSV payloads to find edge cases in parsing (empty strings, Unicode, extremely large numbers, negative scores, NaN/Infinity).
- **Provider C edge cases** — CSV with mixed patient IDs across rows, missing columns, extra columns, different date formats, empty metric values.
- **OTEL integration** — Verify distributed tracing spans are emitted correctly when `OTEL_EXPORTER_OTLP_ENDPOINT` is configured (using a mock OTEL collector).
- **PHI audit coverage** — Automated scanning of all response payloads to ensure no PII/PHI leaks beyond the explicit checks.
- **Performance benchmarks** — Criterion benchmarks for each provider adapter to catch regressions in normalization throughput.
- **Contract tests** — JSON Schema validation on API responses to enforce the unified output contract independently of the Rust types.

## How I Made This Testable

1. **Trait-based provider abstraction** — `NormalizationProvider` is a trait with `validate_input()` and `normalize()` methods. Each provider implements pure transformations with no I/O, making them trivially unit-testable without mocking.

2. **Pipeline as a pure function** — `NormalizationPipeline::run(&provider, &bytes)` is a static method that takes a provider and raw bytes, returning `Result<Vec<UnifiedAssessment>>`. No server, no state, no side effects — just input → output.

3. **Fixture-driven tests** — Real sample data in `fixtures/` matches the PDF specification exactly. Tests use `include_bytes!` / `include_str!` to load them at compile time. Adding a new provider means adding one fixture file and the corresponding test assertions.

4. **Separation of HTTP from business logic** — Handlers are thin: they extract the request, call the pipeline, and format the response. All business logic lives in providers and the pipeline, tested independently. The API integration tests verify HTTP semantics, not transformation correctness.

5. **In-memory state with standard interface** — `AppState` wraps a `DashMap` behind simple `store()` / `get()` / `list()` methods. The `axum-test` `TestServer` gets a fresh `AppState` per test, providing test isolation without external dependencies.

6. **Compose tests as a separate layer** — The shell-based compose tests run against the real Docker image, catching packaging/deployment issues (wrong binary, missing libraries, environment variable misconfiguration) that Rust unit/integration tests cannot.

## Running Tests

```bash
# All Rust tests (18 tests)
cargo test

# Specific test suite
cargo test --test provider_adapter_test
cargo test --test pipeline_test
cargo test --test api_integration_test

# Compose integration tests (48 assertions)
COMPOSE_UP=1 ./tests/compose_integration_test.sh

# Verbose output
cargo test -- --nocapture
```
