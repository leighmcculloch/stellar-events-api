FROM rust:1.93 AS builder
WORKDIR /app

ARG BUILD_REPO=""
ARG BUILD_BRANCH=""
ARG BUILD_COMMIT=""

# Cache dependencies: copy manifests, create stub source, build deps only.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release \
    && rm -rf src

# Build the real binary.
COPY src src
RUN touch src/main.rs src/lib.rs \
    && BUILD_REPO="$BUILD_REPO" BUILD_BRANCH="$BUILD_BRANCH" BUILD_COMMIT="$BUILD_COMMIT" \
       cargo build --release

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false appuser
COPY --from=builder /app/target/release/stellar-events-api /usr/local/bin/
USER appuser
EXPOSE 3000
ENTRYPOINT ["stellar-events-api"]
