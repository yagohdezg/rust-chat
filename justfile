# rust-chat — common developer tasks.
# Run `just` to list recipes. Assumes the Nix dev shell (`nix develop`),
# which provides rustc/cargo, sqlx-cli, postgresql, node/bun and podman.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Compose provider; override with e.g. `just compose="docker compose" stack-up`.
compose := "podman compose"

default:
    @just --list

# ---- build / quality -------------------------------------------------------

# Type-check the whole workspace.
check:
    cargo check --workspace --all-targets

# Lint with clippy, warnings are errors (matches CI).
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Format all Rust sources.
fmt:
    cargo fmt --all

# Verify formatting without writing (CI mode).
fmt-check:
    cargo fmt --all -- --check

# Run the test suite.
test:
    cargo test --workspace

# Run every quality gate the way CI does.
ci: fmt-check lint test
    @echo "all checks passed"

# Build optimized binaries.
build:
    cargo build --workspace --release

# ---- run -------------------------------------------------------------------

# Run the chat API server.
run:
    cargo run -p chat-server

# Run the standalone sandbox execution service.
sandboxd:
    cargo run -p sandboxd

# ---- database --------------------------------------------------------------

# Start only Postgres + pgvector (for `just run` / local dev).
db-up:
    {{compose}} -f deploy/compose.yaml up -d postgres

# Stop the database.
db-down:
    {{compose}} -f deploy/compose.yaml down

# Apply Postgres migrations against $DATABASE_URL (the server also migrates on boot).
migrate:
    sqlx migrate run --source migrations/postgres

# Create the application database/role if it does not exist.
db-create:
    psql "postgres://postgres:postgres@localhost:5432/postgres" -v ON_ERROR_STOP=1 \
        -c "create role rustchat login password 'rustchat';" \
        -c "create database rustchat owner rustchat;" || true

# ---- full stack (compose) --------------------------------------------------

# Build and start postgres + chat-server + web.
stack-up:
    {{compose}} -f deploy/compose.yaml up --build -d

# Stop the whole stack.
stack-down:
    {{compose}} -f deploy/compose.yaml down

# Tail stack logs.
stack-logs:
    {{compose}} -f deploy/compose.yaml logs -f

# Also start the optional sandboxd service (needs an external BoxLite).
stack-sandboxd:
    {{compose}} -f deploy/compose.yaml --profile sandboxd up --build -d

# ---- web (SvelteKit) -------------------------------------------------------

# Install frontend dependencies.
web-install:
    bun install --cwd web

# Run the SvelteKit dev server.
web-dev:
    bun run --cwd web dev

# Type-check the frontend.
web-check:
    bun run --cwd web check

# Build the SvelteKit app for production.
web-build:
    bun run --cwd web build
