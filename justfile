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

# --- Deploy (GCP) ----------------------------------------------------------

project_id := "workflow-497713"
region := "europe-west1"
service := "workflow-backend"
tf_dir := "deploy/terraform"

# Init/refresh terraform providers (add -upgrade to bump versions)
tf-init:
    cd {{tf_dir}} && terraform init

# Preview infra changes (terraform.tfvars auto-loaded)
tf-plan:
    cd {{tf_dir}} && terraform plan

# Apply infra changes
tf-apply:
    cd {{tf_dir}} && terraform apply

# Wipe local plugin cache + re-init (state untouched)
tf-reinit:
    cd {{tf_dir}} && rm -rf .terraform .terraform.lock.hcl && terraform init

# Push app secrets from .env to Secret Manager (containers come from terraform)
secrets-push:
    @for pair in "DATABASE_URL wf-database-url" "GITHUB_TOKEN_ENCRYPTION_KEY wf-github-token-encryption-key" "OPENAI_API_KEY wf-openai-api-key"; do \
        set -- $pair; \
        eval val=\$$1; \
        if [ -z "$val" ]; then echo "ERROR: $1 missing in .env" >&2; exit 1; fi; \
        printf '%s' "$val" | gcloud secrets versions add "$2" --data-file=- --project="{{project_id}}"; \
    done

# Build + deploy: push secrets, then submit Cloud Build with non-secret config from .env
deploy: secrets-push
    @for v in SUPABASE_URL CORS_ORIGINS WEB_APP_URL; do \
        eval val=\$$v; \
        if [ -z "$val" ]; then echo "ERROR: $v missing in .env" >&2; exit 1; fi; \
    done; \
    gcloud builds submit --project="{{project_id}}" --config=cloudbuild.yaml \
        --substitutions="^@^_SUPABASE_URL=${SUPABASE_URL}@_SUPABASE_JWT_AUDIENCE=${SUPABASE_JWT_AUDIENCE:-authenticated}@_CORS_ORIGINS=${CORS_ORIGINS}@_WEB_APP_URL=${WEB_APP_URL}"

# Check which secrets exist on GCP + which env vars the live service has
gcp-check:
    @echo "== Secret Manager (enabled versions) =="; \
    for s in wf-database-url wf-github-token-encryption-key wf-openai-api-key wf-otel-collector-config; do \
        if gcloud secrets describe "$s" --project="{{project_id}}" >/dev/null 2>&1; then \
            n=$(gcloud secrets versions list "$s" --project="{{project_id}}" --filter="state=enabled" --format="value(name)" 2>/dev/null | /usr/bin/grep -c .); \
            if [ "$n" -gt 0 ]; then echo "  OK  $s ($n)"; else echo "  EMPTY  $s (no version — run: just secrets-push)"; fi; \
        else echo "  MISSING  $s"; fi; \
    done; \
    echo "== Live service env ({{service}}) =="; \
    gcloud run services describe {{service}} --region="{{region}}" --project="{{project_id}}" \
        --format='value(spec.template.spec.containers[0].env[].name)' 2>/dev/null \
        | tr ';' '\n' | sed 's/^/  /' || echo "  (service not deployed yet)"
