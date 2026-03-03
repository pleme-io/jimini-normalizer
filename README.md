# jimini-normalizer

Multi-Provider Data Normalization Service — a Rust/Axum REST API that ingests patient assessment data from multiple external sources and normalizes it into a unified schema.

## Quick Start

```bash
# Build and run
cargo run

# Run tests
cargo test

# Server starts on port 8080 (configurable via PORT env var)
```

## API

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/normalize?provider=<name>` | Normalize a single provider payload |
| `POST` | `/normalize/batch` | Normalize multiple records with partial failure support |
| `GET` | `/assessments` | List all stored normalized assessments |
| `GET` | `/assessments/:id` | Get a specific assessment by ID |
| `GET` | `/health` | Health check |

### Example

```bash
# Normalize a Provider A payload (nested JSON, 0-10 scores)
curl -X POST 'http://localhost:8080/normalize?provider=provider_a' \
  -H 'Content-Type: application/json' \
  -d @fixtures/provider_a.json

# Normalize a Provider C payload (CSV, 0-10 scores)
curl -X POST 'http://localhost:8080/normalize?provider=provider_c' \
  -H 'Content-Type: text/csv' \
  --data-binary @fixtures/provider_c.csv
```

## Supported Providers

| Provider | Format | Score Scale | Date Format |
|----------|--------|-------------|-------------|
| `provider_a` | Nested JSON | 0-10 (scaled ×10) | ISO 8601 |
| `provider_b` | Flat key-value JSON | 0-100 (no scaling) | ISO 8601 |
| `provider_c` | CSV | 0-10 (scaled ×10) | MM/DD/YYYY |

## Architecture

### Provider Adapter Pattern

Each external data source is handled by an isolated adapter implementing the `NormalizationProvider` trait:

```rust
trait NormalizationProvider: Send + Sync {
    fn name(&self) -> &str;
    fn format(&self) -> &str;
    fn validate_input(&self, raw: &[u8]) -> Result<(), AppError>;
    fn normalize(&self, raw: &[u8]) -> Result<Vec<UnifiedAssessment>, AppError>;
}
```

Adding a new provider means implementing this trait — no changes to the pipeline or handlers.

### Transform Pipeline

Every normalization request passes through staged processing:

1. **Validate Input** — Provider-specific schema validation (required fields, types, format)
2. **Normalize** — Parse, transform field names, scale scores to 0-100
3. **Validate Output** — Unified schema invariants (score ranges, required fields, non-empty dimensions)

If any stage fails, the pipeline returns a descriptive error with field-level details.

### Unified Output Schema

All providers normalize to the same output:

```json
{
  "id": "uuid",
  "patient_id": "PAT-001",
  "assessment_date": "2024-11-15T10:30:00Z",
  "assessment_type": "PHQ-9",
  "scores": [
    { "dimension": "mood", "value": 75.0, "scale": "0-100" }
  ],
  "metadata": {
    "source_provider": "provider_a",
    "source_format": "json_nested",
    "ingested_at": "2024-11-15T12:00:00Z",
    "version": "1.0"
  }
}
```

### Error Handling

- **400** — Validation errors (with per-field details) or unknown provider
- **404** — Assessment not found
- **422** — Unparseable input format
- **207** — Batch partial failure (successes + errors returned together)
- **500** — Internal errors (no PHI in response body)

### Batch Processing

The `/normalize/batch` endpoint processes multiple records independently. If some succeed and others fail, it returns `207 Multi-Status` with both results:

```json
{
  "successes": [/* normalized assessments */],
  "errors": [{ "index": 1, "provider": "unknown", "error": "..." }]
}
```

## Design Decisions

1. **Trait-based adapters over if/else chains** — Each provider is a self-contained struct. Adding Provider D means one new file, one trait impl, one registry call. No existing code changes.

2. **Staged pipeline with fail-fast** — Input validation runs before any transformation. This catches malformed data early and provides clear error messages rather than cryptic deserialization failures mid-transform.

3. **In-memory storage (DashMap)** — For this scope, a concurrent hash map provides thread-safe reads/writes without database complexity. Production would swap this for a persistent store behind the same interface.

4. **Score normalization at the adapter level** — Each adapter knows its source scale and handles conversion. This keeps scaling logic co-located with format knowledge rather than in a generic normalizer that would need to know about every provider's conventions.

5. **No PHI in logs or error responses** — Patient IDs and names are never included in log output or error response bodies. Errors reference field names and structural issues only.

## Production Considerations

- **Persistence** — Replace `DashMap` with PostgreSQL/Redis for durable storage. The `AppState` abstraction makes this a localized change.
- **Authentication** — Add JWT/API key middleware. The layered router architecture supports inserting auth middleware without touching handlers.
- **Rate limiting** — Tower middleware (`tower::limit::RateLimitLayer`) slots directly into the existing middleware stack.
- **Provider discovery** — The `ProviderRegistry` could load adapters dynamically from a config file or plugin system.
- **Idempotency** — Add idempotency keys to prevent duplicate processing of the same source record.
- **Observability** — Prometheus metrics endpoint, OpenTelemetry distributed tracing, structured JSON logging (already configured).

## Configuration

| Env Var | Default | Description |
|---------|---------|-------------|
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `8080` | Listen port |
| `RUST_LOG` | `info,jimini_normalizer=debug` | Log level filter |
