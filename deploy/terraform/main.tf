provider "google" {
  project = var.project_id
  region  = var.region
}

data "google_project" "project" {
  project_id = var.project_id
}

locals {
  required_services = toset([
    "artifactregistry.googleapis.com",
    "cloudbuild.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "cloudtrace.googleapis.com",
    "iam.googleapis.com",
    "logging.googleapis.com",
    "monitoring.googleapis.com",
    "run.googleapis.com",
    "secretmanager.googleapis.com",
    "serviceusage.googleapis.com",
    "storage.googleapis.com",
    "telemetry.googleapis.com",
  ])

  sccache_bucket_name = var.sccache_bucket_name != "" ? var.sccache_bucket_name : "${var.project_id}-wf-sccache"

  runtime_service_account_email = var.runtime_service_account_email != "" ? var.runtime_service_account_email : "${data.google_project.project.number}-compute@developer.gserviceaccount.com"
  runtime_member                = "serviceAccount:${local.runtime_service_account_email}"

  cloud_build_service_account_email = var.cloud_build_service_account_email != "" ? var.cloud_build_service_account_email : "${data.google_project.project.number}@cloudbuild.gserviceaccount.com"
  cloud_build_member                = "serviceAccount:${local.cloud_build_service_account_email}"
  collector_config_file             = startswith(var.collector_config_path, "/") ? var.collector_config_path : "${path.module}/${var.collector_config_path}"

  runtime_observability_roles = toset([
    "roles/logging.logWriter",
    "roles/monitoring.metricWriter",
    "roles/telemetry.tracesWriter",
  ])

  # Application secret containers (created empty; populate values out-of-band).
  app_secret_ids = toset([
    var.database_url_secret_id,
    var.github_token_encryption_key_secret_id,
  ])
}

resource "google_project_service" "required" {
  for_each = local.required_services

  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}

resource "google_artifact_registry_repository" "app" {
  project       = var.project_id
  location      = var.region
  repository_id = var.artifact_registry_repository_id
  description   = "Cloud Run application images"
  format        = "DOCKER"

  depends_on = [
    google_project_service.required["artifactregistry.googleapis.com"],
  ]
}

resource "google_secret_manager_secret" "collector_config" {
  project   = var.project_id
  secret_id = var.collector_config_secret_id

  replication {
    auto {}
  }

  depends_on = [
    google_project_service.required["secretmanager.googleapis.com"],
  ]
}

resource "google_secret_manager_secret_version" "collector_config" {
  secret      = google_secret_manager_secret.collector_config.id
  secret_data = file(local.collector_config_file)
}

resource "google_secret_manager_secret_iam_member" "runtime_secret_accessor" {
  project   = var.project_id
  secret_id = google_secret_manager_secret.collector_config.id
  role      = "roles/secretmanager.secretAccessor"
  member    = local.runtime_member
}

# Application secrets (DATABASE_URL, GITHUB_TOKEN_ENCRYPTION_KEY). Terraform
# creates only the *containers* — never a secret version — so the sensitive
# values stay out of state and out of the repo. Populate once with, e.g.:
#   printf '%s' "$DATABASE_URL" | gcloud secrets versions add wf-database-url --data-file=- --project=PROJECT
resource "google_secret_manager_secret" "app" {
  for_each = local.app_secret_ids

  project   = var.project_id
  secret_id = each.value

  replication {
    auto {}
  }

  depends_on = [
    google_project_service.required["secretmanager.googleapis.com"],
  ]
}

resource "google_secret_manager_secret_iam_member" "runtime_app_secret_accessor" {
  for_each = google_secret_manager_secret.app

  project   = var.project_id
  secret_id = each.value.id
  role      = "roles/secretmanager.secretAccessor"
  member    = local.runtime_member
}

resource "google_project_iam_member" "runtime_observability" {
  for_each = local.runtime_observability_roles

  project = var.project_id
  role    = each.value
  member  = local.runtime_member
}

resource "google_artifact_registry_repository_iam_member" "cloud_build_writer" {
  count = var.grant_cloud_build_deployer_roles ? 1 : 0

  project    = var.project_id
  location   = google_artifact_registry_repository.app.location
  repository = google_artifact_registry_repository.app.name
  role       = "roles/artifactregistry.writer"
  member     = local.cloud_build_member
}

resource "google_project_iam_member" "cloud_build_run_admin" {
  count = var.grant_cloud_build_deployer_roles ? 1 : 0

  project = var.project_id
  role    = "roles/run.admin"
  member  = local.cloud_build_member
}

resource "google_service_account_iam_member" "cloud_build_runtime_act_as" {
  count = var.grant_cloud_build_deployer_roles ? 1 : 0

  service_account_id = "projects/${var.project_id}/serviceAccounts/${local.runtime_service_account_email}"
  role               = "roles/iam.serviceAccountUser"
  member             = local.cloud_build_member
}

# sccache compiler cache. Cloud Build reads/writes Rust build artifacts here, so
# the cache survives across builds (each build runs on a fresh, cacheless VM).
resource "google_storage_bucket" "sccache" {
  project                     = var.project_id
  name                        = local.sccache_bucket_name
  location                    = var.region
  uniform_bucket_level_access = true
  # Cache-only data; allow `terraform destroy` to remove it even when populated.
  force_destroy = true

  # Evict stale cache objects so the bucket doesn't grow without bound.
  lifecycle_rule {
    condition {
      age = var.sccache_cache_retention_days
    }
    action {
      type = "Delete"
    }
  }

  depends_on = [
    google_project_service.required["storage.googleapis.com"],
  ]
}

# Cloud Build authenticates to GCS as its own service account (via the GCE
# metadata server, exposed to the build by `--driver-opt network=cloudbuild`), so
# it needs read/write on the cache bucket. objectAdmin covers get/list/create.
resource "google_storage_bucket_iam_member" "cloud_build_sccache" {
  count = var.grant_cloud_build_deployer_roles ? 1 : 0

  bucket = google_storage_bucket.sccache.name
  role   = "roles/storage.objectAdmin"
  member = local.cloud_build_member
}
