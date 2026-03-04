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

- **Invalid input rejection:** Non-JSON input fails at the pre-transform boundary validation.
- **Output validation:** Scores outside 0-100 range after scaling (e.g., input 15 on 0-10 scale → 150) are caught by post-transform boundary validation.
- **Happy path:** Valid Provider B input passes all pipeline stages with scores in range.
- **camelCase serialization:** Output JSON uses `patientId`, `assessmentDate`, `sourceProvider` — never snake_case.
- **PHI non-propagation:** Patient names and dates of birth from input fixtures never appear in normalized output.

### API Integration Tests (`tests/api_integration_test.rs`) — 19 tests

Full HTTP round-trips via `axum-test` (requires PostgreSQL via `DATABASE_URL`):

- **Health:** `GET /health` returns `{"status":"ok","database":"ok"}`
- **Normalize all 3 providers:** Each returns 200 with correct unified schema shape, correct `patientId`, `assessmentType`, score counts, and metadata
- **Score scaling:** Provider A and C scores verified at 0-10 → 0-100 (anxiety 7→70, social 4→40, etc.)
- **PHI redaction:** Patient names, DOB, and PHI fields verified absent from all provider outputs
- **camelCase output:** `patientId`, `assessmentType`, `sourceProvider` present; snake_case equivalents absent
- **Unknown provider:** Returns 400
- **Malformed input:** Returns 400 or 422
- **Round-trip persistence:** Normalize → `GET /assessments/{id}` retrieves same assessment by UUID from Postgres
- **List assessments:** `GET /assessments` returns non-empty array after normalization
- **404 for missing:** `GET /assessments/{nonexistent-uuid}` returns 404
- **Batch partial failure:** `POST /normalize/batch` with one valid and one unknown provider returns 207 Multi-Status
- **Metrics:** `GET /metrics` returns Prometheus format with `jimini_http_requests_total` and `jimini_normalizations_total`
- **Failed records:** Malformed input → `GET /failed?provider=provider_a` returns captured failure with full context
- **Audit logs:** Successful normalize → `GET /audit/logs?provider=provider_b` returns audit trail with transformation steps
- **Schema violations:** `GET /audit/violations` returns array

## Boundary Validation Testing

The pipeline now validates at two boundaries. Tests verify:

1. **Pre-transform boundary** — Provider-specific schema validation catches structural issues before transformation:
   - Provider A: missing `patient` or `assessment` object
   - Provider B: missing `patient_id`, `assessment_type`, or no `score_*` keys
   - Provider C: missing CSV columns, invalid UTF-8

2. **Post-transform boundary** — Centralized unified schema validation catches pipeline output issues:
   - Score values outside 0-100 range
   - Empty dimensions, missing assessmentType
   - Wrong scale format (must be "0-100")

3. **Mismatch persistence** — Both boundaries write violations to `schema_violations` table with full raw input for audit and replay.

## Failure Recovery Testing

- **Failed record capture:** Every error path (parse, validation, schema mismatch, unknown provider, internal) results in a row in `failed_records` with the original raw input.
- **Replay:** `POST /failed/{id}/replay` re-runs the pipeline on the stored raw input. On success, the assessment is persisted and the failed record is marked `replayed=true`.
- **Batch failures:** Each item in a batch that fails is independently captured — one bad item doesn't affect others.

## What I'd Test With More Time

- **Concurrency stress tests** — Parallel normalize requests to verify Postgres connection pool behavior under load.
- **Outbox flush worker** — Verify messages drain from outbox to sink, retry on transient failures, and mark as failed after max retries.
- **Retention purge** — Verify `POST /admin/purge` deletes records older than `RETENTION_DAYS` in correct dependency order.
- **Large batch processing** — Batches with 100+ records to verify memory behavior and transaction semantics.
- **Fuzz testing** — Property-based tests (proptest) generating random JSON/CSV payloads to find edge cases.
- **Schema violation replay** — Verify that after fixing a provider adapter bug, previously failed records can be replayed successfully.
- **OTEL integration** — Verify distributed tracing spans are emitted correctly.
- **PHI audit coverage** — Automated scanning of all response payloads to ensure no PII/PHI leaks.
- **Contract tests** — JSON Schema validation on API responses to enforce the unified output contract.

## How I Made This Testable

1. **Trait-based provider abstraction** — `NormalizationProvider` is a trait with `validate_input()` and `normalize()` methods. Each provider implements pure transformations with no I/O, making them trivially unit-testable without mocking.

2. **Pipeline as a pure function** — `NormalizationPipeline::run(&provider, &bytes)` returns `Result<PipelineResult, PipelineError>` with audit metadata and violation context. No server, no state — just input → output + audit trail.

3. **Fixture-driven tests** — Real sample data in `fixtures/` matches the PDF specification exactly. Tests use `include_bytes!` / `include_str!` to load them at compile time.

4. **Separation of HTTP from business logic** — Handlers are thin: they extract the request, call the pipeline, persist results, and format the response. The pipeline and providers are tested independently of HTTP.

5. **Database-backed state** — `AppState` wraps a `sea_orm::DatabaseConnection`. Integration tests connect to a real Postgres instance (via `DATABASE_URL`), providing realistic test conditions.

6. **Compose tests as a separate layer** — Shell-based compose tests run against the real Docker image, catching packaging/deployment issues that Rust tests cannot.

## Running Tests

```bash
# Unit tests (no DB needed) — 10 tests
cargo test --test provider_adapter_test --test pipeline_test

# Integration tests (requires DATABASE_URL) — 19 tests
DATABASE_URL=postgres://jimini:jimini@localhost:5432/jimini cargo test --test api_integration_test

# All Rust tests (29 total)
cargo test

# Via Nix (recommended)
nix run .#test

# Verbose output
cargo test -- --nocapture
```

## Migration Safety

Migrations are classified in `migration-manifest.yaml` and validated at release time via `deploy/normalizer.yaml`:

- All DDL uses `IF NOT EXISTS` / `IF EXISTS` guards for idempotency
- `DROP TABLE` / `DROP COLUMN` / `RENAME` are forbidden without expand-contract pattern
- Indexes use `IF NOT EXISTS` guards
- Release gates validate migration safety before pushing images
