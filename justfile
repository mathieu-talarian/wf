# Workflow backend — task runner. `just` lists recipes.
# .env is loaded into recipe env (DATABASE_URL, ...).

set dotenv-load

default:
    @just --list --unsorted

# --- Build & verify -------------------------------------------------------

build:
    cargo build --workspace

test:
    cargo test --workspace

# The exact CI lint gate — run this, not plain clippy
lint:
    cargo clippy --all --all-targets --locked -- -D warnings

fmt:
    cargo fmt --all

# Everything CI checks: lint gate + full test suite
check: lint test

# --- Run ------------------------------------------------------------------

# Start the API on http://localhost:3000 (.env auto-loaded by the binary too)
run:
    cargo run -p wf-api

# Apply DB migrations (DATABASE_URL must be the session pooler, port 5432)
migrate:
    cargo run -p migration -- up

# --- Event backbone / activity feed --------------------------------------
# The tick runs in-process every TICK_SCHEDULER_SECS (default 120) while
# `just run` is up — no manual trigger needed.

# Live tick smoke test (needs .env + a connected user)
smoke:
    cargo run -p wf-sync --example tick_smoke

# DB-gated tick integration tests (uses DATABASE_URL from .env)
db-tests:
    cargo test -p wf-sync --test tick_db -- --test-threads=1

# --- Live harnesses & misc -------------------------------------------------

# Run a wf-db live example: just example gh_dashboard  (phase0|gh_validate|gh_repo|gh_dashboard|gh_repo_write)
example name:
    cargo run -p wf-db --example {{name}}

# Run SQL against DATABASE_URL: just sql "SELECT * FROM events LIMIT 5"
sql query:
    cargo run -q -p wf-db --example sql -- "{{query}}"

# Dump the OpenAPI spec from the running API
openapi:
    curl -fsS localhost:3000/api/openapi.json

# TS-vs-Rust response parity diff
parity:
    python3 scripts/parity.py

# --- Web client (../workflow) ----------------------------------------------

# Regenerate the web client's API types from the running API
api-sync:
    cd ../workflow && yarn api:spec && yarn api:gen

# Web client gates (it has no test framework)
web-check:
    cd ../workflow && yarn type && yarn lint && yarn build
