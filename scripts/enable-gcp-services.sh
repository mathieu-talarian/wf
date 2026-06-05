#!/usr/bin/env bash
set -euo pipefail

project_id="${1:-${PROJECT_ID:-}}"

if [[ -z "${project_id}" ]]; then
  project_id="$(gcloud config get-value project 2>/dev/null || true)"
fi

if [[ -z "${project_id}" ]]; then
  echo "usage: $0 PROJECT_ID" >&2
  echo "or set PROJECT_ID / gcloud config project" >&2
  exit 1
fi

gcloud services enable \
  artifactregistry.googleapis.com \
  cloudbuild.googleapis.com \
  cloudresourcemanager.googleapis.com \
  cloudtrace.googleapis.com \
  iam.googleapis.com \
  logging.googleapis.com \
  monitoring.googleapis.com \
  run.googleapis.com \
  secretmanager.googleapis.com \
  storage.googleapis.com \
  telemetry.googleapis.com \
  --project="${project_id}"
