# jimini-normalizer

Multi-Provider Data Normalization Service — a Rust/Axum REST API that ingests patient assessment data from multiple external sources, normalizes it into a unified schema, persists to PostgreSQL with full audit trails, and flushes to downstream sinks via a mailbox pattern.

## Quick Start

```bash
# Start Postgres for local dev
docker compose up -d

# Run the service (requires Postgres at DATABASE_URL)
cargo run

# Run unit tests (no DB needed) — 10 tests
cargo test --test provider_adapter_test --test pipeline_test

# Run integration tests (requires DATABASE_URL) — 19 tests
DATABASE_URL=postgres://jimini:jimini@localhost:5432/jimini cargo test --test api_integration_test

# Run all tests
cargo test
```

## Release & Build Commands

All lifecycle commands are available via Nix (provided by substrate's `rust-service-flake.nix`):

```bash
# Build Docker images (amd64 + arm64)
nix run .#build

# Push images to GHCR
nix run .#push

# Full multi-arch release (build + push + tag with git SHA)
nix run .#release

# Deploy to K8s (via forge orchestrate-release)
nix run .#deploy

# Monitor K8s rollout
nix run .#rollout

# Run tests / lint / format
nix run .#test
nix run .#lint
nix run .#fmt

# Regenerate Cargo.nix (after dependency changes)
nix run .#regenerate

# Local dev (docker-compose + migrations + cargo run)
nix run .#dev
nix run .#dev-down
```

### Image Tagging

Images are tagged with `{arch}-{git-sha-short}` format:
- `ghcr.io/pleme-io/jimini-normalizer:amd64-abc1234`
- `ghcr.io/pleme-io/jimini-normalizer:arm64-abc1234`
- `ghcr.io/pleme-io/jimini-normalizer:abc1234` (multi-arch manifest)

### Migration Safety

Migrations follow the expand-contract pattern for zero-downtime deployments:

```
DROP/RENAME column?  → FORBIDDEN without expand-contract (3-phase)
ADD nullable column? → Direct add OK (use IF NOT EXISTS)
ADD NOT NULL column? → 3-step: nullable → backfill → SET NOT NULL
ADD index?           → IF NOT EXISTS guard
```

See `migration-manifest.yaml` for migration classification and `deploy/normalizer.yaml` for release gate configuration.

## API

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/normalize?provider=<name>` | Normalize a single provider payload |
| `POST` | `/normalize/batch` | Normalize multiple records with partial failure support |
| `GET` | `/assessments` | List all stored normalized assessments |
| `GET` | `/assessments/{id}` | Get a specific assessment by ID |
| `GET` | `/health` | Health check (includes DB status) |
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/audit/logs` | Query normalization audit trail |
| `GET` | `/audit/violations` | Query schema boundary violations |
| `GET` | `/failed` | List failed records (filterable by provider, error_kind, replayed) |
| `GET` | `/failed/{id}` | Get a specific failed record with full context |
| `POST` | `/failed/{id}/replay` | Replay a failed record through the pipeline |
| `POST` | `/admin/purge` | Trigger retention purge (called by K8s CronJob) |

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

# Query audit trail for a provider
curl 'http://localhost:8080/audit/logs?provider=provider_a'

# List failed records that haven't been replayed
curl 'http://localhost:8080/failed?replayed=false'

# Replay a failed record
curl -X POST 'http://localhost:8080/failed/<uuid>/replay'
```

## Supported Providers

| Provider | Format | Score Scale | Date Format |
|----------|--------|-------------|-------------|
| `provider_a` | Nested JSON | 0-10 (scaled x10) | ISO 8601 |
| `provider_b` | Flat key-value JSON | 0-100 (no scaling) | ISO 8601 |
| `provider_c` | CSV | 0-10 (scaled x10) | YYYY-MM-DD |

## Architecture

### Data Flow

```
Raw Input → Pre-Transform Schema Validation → Provider Adapter → Post-Transform Schema Validation
                   ↓ (mismatch)                                        ↓ (mismatch)
            schema_violations table                            schema_violations table
                                                                       ↓ (success)
                                                              ┌─ assessments table
                                                    Postgres ─┤─ normalization_audit_log
                                                              ├─ outbox (pending)
                                                              └─ failed_records (on error)
                                                                       ↓
                                                              Flush Worker (async)
                                                                       ↓
                                                              Vector / OTEL Sink
                                                                       ↓
                                                              Downstream Systems

                   K8s CronJob (daily) → POST /admin/purge → 90-day retention cleanup
```

### Boundary Schema Validation

Validation happens at two boundaries:

1. **Pre-transform** — Validates raw input against provider-specific schemas before any transformation. Catches structural issues (missing fields, wrong types) at the source boundary. Mismatches are persisted to `schema_violations` with `boundary=pre_transform`.

2. **Post-transform** — Validates the normalized output against the centralized unified schema after transformation. Catches pipeline bugs (score out of range, empty dimensions, wrong scale). Mismatches are persisted to `schema_violations` with `boundary=post_transform`.

Both boundaries capture the full raw input alongside violation details, enabling replay after fixes.

### Mailbox Pattern (Outbox)

Normalized assessments are not sent directly to downstream systems. Instead:

1. **Write to Postgres** — Assessment + audit log + outbox entry in a single transaction
2. **Flush Worker** — Background async worker polls the `outbox` table every N seconds
3. **Sink to Vector/OTEL** — Each pending message is POSTed to the configured sink endpoint
4. **Status tracking** — `pending` → `flushed` (or `failed` after max retries)

This guarantees at-least-once delivery and decouples the normalize API from downstream availability.

### Failure Recovery

All failures are captured in the `failed_records` table with:
- Full raw input (for replay)
- Error classification (`parse`, `validation`, `schema_mismatch`, `internal`)
- Structured error details (JSONB)
- SHA-256 input hash (deduplication)
- Replay tracking (`replayed`, `replayed_at`)

Failed records can be explored via `GET /failed` and replayed via `POST /failed/{id}/replay`.

### 90-Day Data Retention

A Kubernetes CronJob runs daily at 03:00 UTC, calling `POST /admin/purge`. This deletes all records older than the configured `RETENTION_DAYS` (default: 90) across all tables in dependency order. Any data that hit the service in the last 90 days is recoverable.

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

1. **Pre-Transform Schema Validation** — Structural check against provider-specific schema
2. **Validate Input** — Provider-specific deep validation (required fields, types, format)
3. **Normalize** — Parse, transform field names, scale scores to 0-100
4. **Post-Transform Schema Validation** — Structural check against unified output schema
5. **Validate Output** — Unified schema invariants (score ranges, required fields, non-empty dimensions)

Each stage records timing metadata for the audit trail. If any stage fails, the full error context is persisted to `failed_records` for later exploration and replay.

### Error Handling

- **400** — Validation errors (with per-field details) or unknown provider
- **404** — Assessment not found
- **422** — Unparseable input format
- **207** — Batch partial failure (successes + errors returned together)
- **500** — Internal/database errors (no PHI in response body)

### Secrets Management

Secrets (DATABASE_URL, SINK_ENDPOINT) are managed via SOPS-encrypted Kubernetes Secrets:

- **Encryption**: Age key (`age1q3tep...`) or GCP KMS backend
- **Pattern**: `encrypted_regex: "^(data|stringData)$"` — only secret values encrypted, metadata stays plaintext for Kustomize transformers
- **Decryption**: FluxCD kustomize-controller reads the `sops-age` Secret in flux-system namespace
- **Age key backends**:
  - Local: age private key at `~/.config/sops/age/keys.txt`
  - GCP KMS: set `gcp_kms` in `.sops.yaml` or `SOPS_GCP_KMS_IDS` env var

## Design Decisions

1. **PostgreSQL over Redis for persistence** — Audit data needs durability, queryability (JSONB, date ranges, joins), and append-only semantics. Redis is for caching/ephemeral state. Postgres is the right choice for a compliance-focused service.

2. **Scores as JSONB, not a separate table** — Scores are always read/written together with the assessment. No query pattern requires cross-dimension aggregation. JSONB avoids JOINs.

3. **Mailbox pattern over direct push** — Decouples the normalize API from downstream sink availability. The outbox table provides at-least-once delivery with retry tracking.

4. **All failures persisted** — Every error (parse, validation, schema mismatch, internal) is captured with full raw input. This enables exploration of error patterns and replay after bug fixes.

5. **Boundary validation at both edges** — Pre-transform catches bad inputs before wasting compute. Post-transform catches pipeline bugs before persisting bad data. Both write to `schema_violations` for auditing.

6. **90-day retention via CronJob** — Data lifecycle managed at the infrastructure level. Any data within the window is recoverable; older data is purged to control storage costs.

7. **No PHI in logs or error responses** — Patient IDs and names are never included in log output or error response bodies.

## Configuration

| Env Var | Default | Description |
|---------|---------|-------------|
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `8080` | Listen port |
| `RUST_LOG` | `info,jimini_normalizer=debug` | Log level filter |
| `DATABASE_URL` | `postgres://jimini:jimini@localhost:5432/jimini` | PostgreSQL connection string |
| `SINK_ENDPOINT` | `http://localhost:8686` | Vector/OTEL HTTP sink for flushed messages |
| `FLUSH_INTERVAL_SECS` | `5` | Outbox flush worker poll interval |
| `RETENTION_DAYS` | `90` | Data retention window (days) |

## Database Schema

### Tables

| Table | Purpose |
|-------|---------|
| `assessments` | Normalized assessment records (JSONB scores) |
| `normalization_audit_log` | Per-assessment transformation audit trail |
| `schema_violations` | Boundary validation mismatches (pre/post transform) |
| `outbox` | Mailbox for async flush to sink (pending/flushed/failed) |
| `failed_records` | All failure data with raw input for replay |

Migrations run automatically on startup via SeaORM.

## Deployment Architecture

### Nix Build Pipeline

Production Docker images are built via Nix (not Dockerfile):

```
flake.nix (serviceType = "rest")
  → substrate/lib/rust-service-flake.nix
    → crate2nix-builders.nix (per-crate granular caching, musl static linking)
    → crate2nix-apps.nix (nix run .#release, .#build, .#push, etc.)
    → dockerTools.buildLayeredImage (minimal: cacert + static binary only)
```

### Minimal Production Image

The Nix-built image is hardened for production:
- **Static musl binary** — no glibc, no shared libraries
- **No shell** — no bash, sh, or package manager
- **No curl/wget** — no network tools for exfiltration
- **No openssl shared lib** — uses rustls (pure Rust TLS), not native openssl
- **Runs as nobody** (UID 65534) — no root access
- **Read-only root filesystem** — enforced by K8s securityContext
- **Contents**: only `cacert` (TLS certificates) + the service binary

There is no Dockerfile — local dev runs `docker compose up -d` for Postgres, then `cargo run`.

### FluxCD GitOps

Deployment uses FluxCD with OCI-published Helm charts:

```
Product repo (jimini-normalizer)          k8s repo
================================          ================================
chart/jimini-normalizer/                  clusters/plo/services/jimini-normalizer/
  Chart.yaml (pleme-lib dependency)         helmrelease.yaml (HelmRelease CR)
  values.yaml (defaults)                      chart: jimini-normalizer
  templates/ (pleme-lib includes)             values.image.tag: amd64-{sha}

deploy/flux/                              shared/infrastructure/pleme-charts/
  helmrelease.yaml (reference)              helm-repository.yaml (OCI → ghcr.io)
  kustomization.yaml
  namespace.yaml

deploy/secrets/
  database-secret.yaml (SOPS-encrypted)
  sink-secret.yaml (SOPS-encrypted)
```

Image tags are pinned in the k8s repo's HelmRelease `values.image.tag` field and updated via git commits. Chart version and image version are decoupled — chart changes when templates change, image changes on every code release.
