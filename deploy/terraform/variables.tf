variable "project_id" {
  description = "Google Cloud project ID."
  type        = string
}

variable "region" {
  description = "Google Cloud region for Artifact Registry and Cloud Run."
  type        = string
  default     = "europe-west1"
}

variable "artifact_registry_repository_id" {
  description = "Artifact Registry Docker repository ID used by Cloud Build."
  type        = string
  default     = "workflow-backend"
}

variable "collector_config_secret_id" {
  description = "Secret Manager secret ID that stores the collector config."
  type        = string
  default     = "wf-otel-collector-config"
}

variable "collector_config_path" {
  description = "Path to the OpenTelemetry Collector config file."
  type        = string
  default     = "../../otel-config.yaml"
}

variable "database_url_secret_id" {
  description = "Secret Manager secret ID holding DATABASE_URL (Supabase session pooler). Container is created empty; populate the value out-of-band."
  type        = string
  default     = "wf-database-url"
}

variable "github_token_encryption_key_secret_id" {
  description = "Secret Manager secret ID holding GITHUB_TOKEN_ENCRYPTION_KEY (base64 32 bytes). Container is created empty; populate the value out-of-band."
  type        = string
  default     = "wf-github-token-encryption-key"
}

variable "runtime_service_account_email" {
  description = "Cloud Run runtime service account email. Defaults to the project's Compute Engine default service account."
  type        = string
  default     = ""
}

variable "cloud_build_service_account_email" {
  description = "Cloud Build service account email to grant deploy permissions to. Defaults to PROJECT_NUMBER@cloudbuild.gserviceaccount.com."
  type        = string
  default     = ""
}

variable "grant_cloud_build_deployer_roles" {
  description = "Grant the Cloud Build service account permissions to push images and deploy Cloud Run."
  type        = bool
  default     = true
}

variable "sccache_bucket_name" {
  description = "GCS bucket name for the Cloud Build sccache compiler cache. Empty defaults to \"<project_id>-wf-sccache\". Pass the value to Cloud Build as the _SCCACHE_BUCKET substitution."
  type        = string
  default     = ""
}

variable "sccache_cache_retention_days" {
  description = "Age in days after which sccache cache objects are auto-deleted by a bucket lifecycle rule."
  type        = number
  default     = 30
}
