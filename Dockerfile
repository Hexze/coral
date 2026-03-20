# syntax=docker/dockerfile:1

# Build stage
FROM rust:1.92-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifest and lockfile
COPY Cargo.docker.toml ./Cargo.toml
COPY Cargo.lock ./

# Copy all workspace crates
COPY crates/hypixel ./crates/hypixel
COPY crates/blacklist ./crates/blacklist
COPY crates/database ./crates/database
COPY crates/coral-redis ./crates/coral-redis
COPY crates/render ./crates/render
COPY crates/coral-api ./crates/coral-api
COPY crates/coral-bot ./crates/coral-bot
COPY crates/coral-admin ./crates/coral-admin
COPY crates/migration ./crates/migration

# Build all binaries (GIT_AUTH_TOKEN allows cargo to clone private deps)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    --mount=type=secret,id=git_auth_token \
    git config --global url."https://$(cat /run/secrets/git_auth_token)@github.com/".insteadOf "https://github.com/" \
    && cargo build --release --bin coral-api --bin coral-bot --bin coral-admin \
    && cp target/release/coral-api target/release/coral-bot target/release/coral-admin /usr/local/bin/ \
    && git config --global --unset url."https://$(cat /run/secrets/git_auth_token)@github.com/".insteadOf

# Runtime stage for coral-api
FROM debian:bookworm-slim AS coral-api

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/coral-api /usr/local/bin/

ENV RUST_LOG=info
EXPOSE 8000

CMD ["coral-api"]

# Runtime stage for coral-bot
FROM debian:bookworm-slim AS coral-bot

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/coral-bot /usr/local/bin/

ENV RUST_LOG=info

CMD ["coral-bot"]

# Runtime stage for coral-admin
FROM debian:bookworm-slim AS coral-admin

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/coral-admin /usr/local/bin/

ENV RUST_LOG=info
EXPOSE 8080

CMD ["coral-admin"]

# Postgres with migrations baked in
FROM postgres:16-alpine AS coral-postgres

COPY migrations/*.sql /docker-entrypoint-initdb.d/
