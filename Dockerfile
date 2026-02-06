# Build stage
FROM rust:1.83-bookworm AS builder

WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

# Build the actual application
COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/stellar-events-api /usr/local/bin/stellar-events-api

RUN useradd -r -s /bin/false appuser
RUN mkdir -p /data && chown appuser:appuser /data
USER appuser

ENV DATA_DIR=/data
ENV PORT=3000
ENV RUST_LOG=info

EXPOSE 3000

ENTRYPOINT ["stellar-events-api"]
