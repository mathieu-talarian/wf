output "artifact_registry_repository" {
  description = "Artifact Registry Docker repository."
  value       = google_artifact_registry_repository.app.name
}

output "collector_config_secret_id" {
  description = "Secret Manager secret ID mounted by Cloud Run."
  value       = google_secret_manager_secret.collector_config.secret_id
}

output "app_secret_ids" {
  description = "Secret Manager secret IDs for the app (populate their values out-of-band)."
  value       = [for s in google_secret_manager_secret.app : s.secret_id]
}

output "cloud_build_service_account_email" {
  description = "Cloud Build service account granted deployment permissions."
  value       = local.cloud_build_service_account_email
}

output "runtime_service_account_email" {
  description = "Cloud Run runtime service account granted collector permissions."
  value       = local.runtime_service_account_email
}

output "sccache_bucket_name" {
  description = "GCS bucket for the Cloud Build sccache compiler cache. Pass to Cloud Build as _SCCACHE_BUCKET."
  value       = google_storage_bucket.sccache.name
}
