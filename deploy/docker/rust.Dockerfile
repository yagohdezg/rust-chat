# syntax=docker/dockerfile:1
#
# Build any workspace binary (chat-server, sandboxd, ...).
#
# The build context must be the repository root, e.g.:
#   docker build -f deploy/docker/rust.Dockerfile --build-arg SERVICE=chat-server .
#
# Migrations are embedded at compile time via sqlx::migrate!, so `migrations/`
# must be part of the context. SQLite is compiled from source (`bundled`), so no
# system SQLite is required.

FROM docker.io/library/rust:1-bookworm AS builder
ARG SERVICE=chat-server
WORKDIR /src

# Copy manifests plus every workspace member so cargo can resolve the build.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY services ./services
COPY migrations ./migrations

RUN cargo build --release -p "${SERVICE}" \
 && cp "target/release/${SERVICE}" /usr/local/bin/app

FROM docker.io/library/debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/app /usr/local/bin/app

ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/app"]
