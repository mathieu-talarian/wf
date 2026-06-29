# Deploying `wf-api` to Cloud Run

`wf-api` runs on Cloud Run as a **two-container service**: the app plus an
**OpenTelemetry Collector sidecar** (gRPC OTLP on `localhost:4317`) that fans
traces, metrics, and logs out to Cloud Trace, Cloud Monitoring (Managed
Prometheus), and Cloud Logging. Infrastructure is Terraform; the build+deploy is
Cloud Build.

Target project: `workflow-497713` / region `europe-west1` (override via vars/subs).
All cloud resources are **wf-named and isolated** so they never collide with the
sibling `otlp` reference repo in the same project.

## Components

| File | Purpose |
|------|---------|
| `Dockerfile` | Multi-stage build (cargo-chef + sccache) of the `wf-api` binary; slim runtime **with `ca-certificates`** (wf-api makes outbound HTTPS to GitHub/Jira/Supabase/Postgres). |
| `otel-config.yaml` | Collector pipeline config (stored in Secret Manager, mounted by the sidecar). |
| `service.yaml` | Knative manifest: app + collector containers, env, secrets, `/healthz` probes. |
| `cloudbuild.yaml` | Build & push image → render `service.yaml` → `gcloud run services replace`. |
| `deploy/terraform/` | APIs, Artifact Registry, secrets, IAM, sccache bucket, RED dashboard + latency SLO/alerts. |
| `scripts/enable-gcp-services.sh` | One-shot `gcloud services enable` for the required APIs. |

## One-time setup

```bash
# 1. Enable APIs (or let Terraform do it).
./scripts/enable-gcp-services.sh workflow-497713

# 2. Provision infra (Artifact Registry, secrets, IAM, sccache bucket, dashboard).
cd deploy/terraform
cp terraform.tfvars.example terraform.tfvars   # edit if needed
terraform init
terraform apply
export SCCACHE_BUCKET="$(terraform output -raw sccache_bucket_name)"
cd ../..

# 3. Populate the app secret values (Terraform creates only empty containers).
#    DATABASE_URL MUST be the Supabase *session* pooler (…pooler.supabase.com:5432) —
#    the transaction pooler (6543) breaks SeaORM/sqlx.
printf '%s' 'postgres://USER:PASS@aws-0-REGION.pooler.supabase.com:5432/postgres' \
  | gcloud secrets versions add wf-database-url --data-file=- --project=workflow-497713
printf '%s' 'BASE64_32_BYTE_KEY' \
  | gcloud secrets versions add wf-github-token-encryption-key --data-file=- --project=workflow-497713
```

## Deploy

```bash
export SCCACHE_BUCKET="$(terraform -chdir=deploy/terraform output -raw sccache_bucket_name)"

gcloud builds submit --config cloudbuild.yaml \
  --project workflow-497713 \
  --substitutions=_SCCACHE_BUCKET="${SCCACHE_BUCKET}",_SUPABASE_URL=https://YOURPROJ.supabase.co,_CORS_ORIGINS=https://app.example.com,_WEB_APP_URL=https://app.example.com
```

`DATABASE_URL` and `GITHUB_TOKEN_ENCRYPTION_KEY` are injected from Secret Manager
at runtime (see `service.yaml`); the non-secret config (`SUPABASE_URL`,
`CORS_ORIGINS`, `WEB_APP_URL`, `SUPABASE_JWT_AUDIENCE`) is rendered from the Cloud
Build substitutions above.

## Enable alerts (after first traffic)

Cloud Monitoring validates alert/SLO queries against metric descriptors that only
exist once the app has reported data. So deploy, send a few requests, then:

```bash
terraform -chdir=deploy/terraform apply -var enable_alerts=true
# optionally: -var 'notification_channels=["projects/.../notificationChannels/123"]'
```

## Verify

- **Cloud Run**: the revision goes healthy once the `/healthz` startup probe passes.
- **Cloud Trace**: a request to any `/api/...` route produces a `GET /api/...` trace
  (continued from Cloud Run's `traceparent`).
- **Cloud Monitoring**: `http_server_request_duration_seconds` + the
  "workflow-backend — RED" dashboard populate.
- **Cloud Logging**: app logs arrive via the collector's logs pipeline.

## Tick scheduling

The tick runs **in-process**: the server spawns a background task at boot
that calls `wf_sync::run_tick` immediately (startup reconciliation) and then
every `TICK_SCHEDULER_SECS` (default 120 s), logging a `tick.scheduler` line
per run (`scopes_claimed`, `scopes_ok`, `scopes_failed`, `events_written`,
`elapsed_ms`). There is no HTTP trigger and no `INTERNAL_TICK_TOKEN` secret
anymore.

The service keeps Cloud Run's default scaling (scale-to-zero, CPU throttled
between requests), so syncing is deliberately **best-effort**:

- a cold start (first request after idle) boots the server and runs a tick
  right away, catching up on everything that happened while it was down;
- while the instance is handling traffic — i.e. while someone is actually
  using the app — the 2-minute ticks keep running;
- when the service idles or scales to zero, ticks stall until the next
  request. No users online ⇒ no syncing, and that's fine: the backlog is
  reconciled on the next startup tick.

If continuous background syncing ever becomes a requirement, pin
`autoscaling.knative.dev/minScale: "1"` and
`run.googleapis.com/cpu-throttling: "false"` in `service.yaml` (always-on
billing). Concurrent ticks (e.g. during a deploy's instance overlap) are
safe: scope claiming uses `FOR UPDATE SKIP LOCKED` leases and event inserts
dedup. Failed scopes back off automatically; abandoned leases expire after
`TICK_LEASE_SECS` (default 90 s) and are reclaimable by the next tick.

**Migrating an existing deployment:** delete the old trigger and secret —

```bash
gcloud scheduler jobs delete wf-tick --project=workflow-497713
gcloud secrets delete wf-internal-tick-token --project=workflow-497713
```

## Local development

No collector required. `cargo run -p wf-api` (with `.env`) starts the server; with
no `OTEL_EXPORTER_OTLP_ENDPOINT` set, **no OTLP exporters are built** and logs are
**pretty-printed to stdout**. To see telemetry locally, run a collector with a gRPC
OTLP receiver on `:4317` and set `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317`.
