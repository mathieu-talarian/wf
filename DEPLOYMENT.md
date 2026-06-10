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

# 3. Populate the app secret values (Terraform creates only empty containers).
#    DATABASE_URL MUST be the Supabase *session* pooler (…pooler.supabase.com:5432) —
#    the transaction pooler (6543) breaks SeaORM/sqlx.
printf '%s' 'postgres://USER:PASS@aws-0-REGION.pooler.supabase.com:5432/postgres' \
  | gcloud secrets versions add wf-database-url --data-file=- --project=workflow-497713
printf '%s' 'BASE64_32_BYTE_KEY' \
  | gcloud secrets versions add wf-github-token-encryption-key --data-file=- --project=workflow-497713
printf '%s' "$(openssl rand -hex 32)" \
  | gcloud secrets versions add wf-internal-tick-token --data-file=- --project=workflow-497713
```

## Deploy

```bash
gcloud builds submit --config cloudbuild.yaml \
  --project workflow-497713 \
  --substitutions=_SUPABASE_URL=https://YOURPROJ.supabase.co,_CORS_ORIGINS=https://app.example.com,_WEB_APP_URL=https://app.example.com
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

`POST /internal/tick` is called by Cloud Scheduler every 2 minutes. The
`INTERNAL_TICK_TOKEN` env var is sourced from a Secret Manager secret named
`wf-internal-tick-token`. Terraform creates the secret container (alongside
`wf-database-url` and `wf-github-token-encryption-key`), and `service.yaml`
injects it into the container as a `secretKeyRef` with the same pattern as
`DATABASE_URL`.

Add the secret value once (after `terraform apply`):

```bash
printf '%s' "$(openssl rand -hex 32)" \
  | gcloud secrets versions add wf-internal-tick-token --data-file=- --project=workflow-497713
```

Create the Cloud Scheduler job (run once after first deploy):

```bash
gcloud scheduler jobs create http wf-tick \
  --schedule="*/2 * * * *" \
  --uri="https://<service-url>/internal/tick" \
  --http-method=POST \
  --headers="X-Internal-Token=<token>" \
  --attempt-deadline=60s
```

The endpoint returns a JSON summary (`scopes_claimed`, `events_written`,
`errors`) visible in Scheduler logs. Failed scopes back off automatically;
abandoned leases expire after `TICK_LEASE_SECS` (default 90 s) and are
reclaimable by the next tick.

## Local development

No collector required. `cargo run -p wf-api` (with `.env`) starts the server; with
no `OTEL_EXPORTER_OTLP_ENDPOINT` set, **no OTLP exporters are built** and logs are
**pretty-printed to stdout**. To see telemetry locally, run a collector with a gRPC
OTLP receiver on `:4317` and set `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317`.
