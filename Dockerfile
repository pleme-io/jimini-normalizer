FROM rust:1.85-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy manifests first for layer caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main to cache dependencies
RUN mkdir src && \
    echo 'fn main() {}' > src/main.rs && \
    echo 'pub mod config; pub mod error; pub mod handler; pub mod model; pub mod observability; pub mod pipeline; pub mod provider; pub mod router; pub mod state;' > src/lib.rs && \
    mkdir -p src/model src/provider src/handler && \
    touch src/config.rs src/error.rs src/state.rs src/pipeline.rs src/observability.rs src/router.rs && \
    touch src/model/mod.rs src/model/unified.rs src/model/provider_a.rs src/model/provider_b.rs src/model/provider_c.rs && \
    touch src/provider/mod.rs src/provider/provider_a.rs src/provider/provider_b.rs src/provider/provider_c.rs && \
    touch src/handler/mod.rs src/handler/health.rs src/handler/normalize.rs src/handler/assessments.rs && \
    cargo build --release 2>/dev/null || true

# Remove dummy build artifacts (keep cached dependencies)
RUN rm -rf src/ target/release/.fingerprint/jimini* target/release/deps/jimini* target/release/jimini*

# Copy actual source
COPY src/ src/

# Build the actual binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

RUN groupadd -r app && useradd -r -g app -s /sbin/nologin app

COPY --from=builder /app/target/release/jimini-normalizer /usr/local/bin/jimini-normalizer

USER app

ENV PORT=8080
ENV RUST_LOG=info,jimini_normalizer=debug

EXPOSE 8080

ENTRYPOINT ["jimini-normalizer"]
