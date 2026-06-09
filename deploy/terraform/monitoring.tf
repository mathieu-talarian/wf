# Cloud Monitoring: a RED dashboard, SLO-style alert policies, and a latency SLO,
# all built on the OTel HTTP server metrics wf-api emits (via Managed Prometheus).
#
# Metric names below are the Managed-Prometheus form of the OTel instruments
# (dots -> underscores; histogram exposes _count / _bucket / _sum series):
#   http.server.request.duration  -> http_server_request_duration_seconds
#   http.server.active_requests   -> http_server_active_requests
#
# NOTE: `terraform validate` checks schema, not whether the metric/series names
# exist in your project — those only appear after the app has reported data once.

variable "notification_channels" {
  description = "Notification channel IDs for alert policies (empty = create policies that notify nobody)."
  type        = list(string)
  default     = []
}

variable "enable_monitoring" {
  description = "Create the dashboard (safe to apply any time — dashboards don't validate metric existence)."
  type        = bool
  default     = true
}

# Cloud Monitoring validates BOTH alert-policy PromQL queries AND the SLO's
# distribution_filter against existing metric descriptors at *creation* time, and
# those descriptors only exist AFTER the app has been deployed and reported data
# once. So every metric-dependent resource (alerts + SLO + burn-rate) is gated off
# by default: apply infra+dashboard, deploy the app, send a few requests, then
# `terraform apply -var enable_alerts=true`.
variable "enable_alerts" {
  description = "Create alert policies + the latency SLO + burn-rate alerts. Enable only after the app has reported metrics."
  type        = bool
  default     = false
}

# --- RED dashboard ----------------------------------------------------------

resource "google_monitoring_dashboard" "app" {
  count   = var.enable_monitoring ? 1 : 0
  project = var.project_id

  dashboard_json = jsonencode({
    displayName = "workflow-backend — RED"
    mosaicLayout = {
      columns = 12
      tiles = [
        {
          xPos = 0, yPos = 0, width = 6, height = 4
          widget = {
            title = "Request rate (req/s)"
            xyChart = { dataSets = [{
              timeSeriesQuery = { prometheusQuery = "sum(rate(http_server_request_duration_seconds_count[5m]))" }
            }] }
          }
        },
        {
          xPos = 6, yPos = 0, width = 6, height = 4
          widget = {
            title = "5xx error ratio"
            xyChart = { dataSets = [{
              timeSeriesQuery = { prometheusQuery = "sum(rate(http_server_request_duration_seconds_count{http_response_status_code=~\"5..\"}[5m])) / clamp_min(sum(rate(http_server_request_duration_seconds_count[5m])), 1)" }
            }] }
          }
        },
        {
          xPos = 0, yPos = 4, width = 12, height = 4
          widget = {
            title = "Latency p50 / p95 / p99 (s)"
            xyChart = { dataSets = [
              { timeSeriesQuery = { prometheusQuery = "histogram_quantile(0.50, sum by (le) (rate(http_server_request_duration_seconds_bucket[5m])))" } },
              { timeSeriesQuery = { prometheusQuery = "histogram_quantile(0.95, sum by (le) (rate(http_server_request_duration_seconds_bucket[5m])))" } },
              { timeSeriesQuery = { prometheusQuery = "histogram_quantile(0.99, sum by (le) (rate(http_server_request_duration_seconds_bucket[5m])))" } },
            ] }
          }
        },
        {
          xPos = 0, yPos = 8, width = 12, height = 4
          widget = {
            title = "In-flight HTTP requests"
            xyChart = { dataSets = [{
              timeSeriesQuery = { prometheusQuery = "sum(http_server_active_requests)" }
            }] }
          }
        },
      ]
    }
  })
}

# --- Alert policies (SLO-style) ---------------------------------------------

resource "google_monitoring_alert_policy" "high_error_rate" {
  count        = var.enable_alerts ? 1 : 0
  project      = var.project_id
  display_name = "workflow-backend: 5xx error ratio > 5%"
  combiner     = "OR"

  conditions {
    display_name = "5xx ratio > 5% for 5m"
    condition_prometheus_query_language {
      query               = "sum(rate(http_server_request_duration_seconds_count{http_response_status_code=~\"5..\"}[5m])) / clamp_min(sum(rate(http_server_request_duration_seconds_count[5m])), 1) > 0.05"
      duration            = "300s"
      evaluation_interval = "60s"
    }
  }

  notification_channels = var.notification_channels
}

resource "google_monitoring_alert_policy" "high_latency" {
  count        = var.enable_alerts ? 1 : 0
  project      = var.project_id
  display_name = "workflow-backend: p95 latency > 1s"
  combiner     = "OR"

  conditions {
    display_name = "p95 latency > 1s for 5m"
    condition_prometheus_query_language {
      query               = "histogram_quantile(0.95, sum by (le) (rate(http_server_request_duration_seconds_bucket[5m]))) > 1"
      duration            = "300s"
      evaluation_interval = "60s"
    }
  }

  notification_channels = var.notification_channels
}

# --- Latency SLO ------------------------------------------------------------
# distribution_cut treats requests landing within [.., max] seconds as "good".

resource "google_monitoring_custom_service" "app" {
  count        = var.enable_alerts ? 1 : 0
  project      = var.project_id
  service_id   = "workflow-backend"
  display_name = "workflow-backend"
}

resource "google_monitoring_slo" "latency" {
  count        = var.enable_alerts ? 1 : 0
  project      = var.project_id
  service      = google_monitoring_custom_service.app[0].service_id
  slo_id       = "latency-500ms"
  display_name = "95% of requests < 500ms (28d rolling)"

  goal                = 0.95
  rolling_period_days = 28

  request_based_sli {
    distribution_cut {
      distribution_filter = "metric.type=\"prometheus.googleapis.com/http_server_request_duration_seconds/histogram\" resource.type=\"prometheus_target\""
      range {
        max = 0.5
      }
    }
  }
}

# Multi-window, multi-burn-rate alerting on the latency SLO (Google SRE workbook):
#   * Fast burn — 1h window at >=14.4x burns 2% of the 28d budget in an hour → page.
#   * Slow burn — 6h window at >=6x burns 5% of the budget in six hours → investigate.
# Burn rate is read straight from the SLO object via select_slo_burn_rate, so it
# always matches the SLO definition (no re-deriving "good" from raw buckets).
resource "google_monitoring_alert_policy" "slo_burn_rate" {
  count        = var.enable_alerts ? 1 : 0
  project      = var.project_id
  display_name = "workflow-backend: latency SLO burn rate"
  combiner     = "OR"

  conditions {
    display_name = "Fast burn (1h ≥ 14.4x)"
    condition_monitoring_query_language {
      query    = <<-EOT
        select_slo_burn_rate("${google_monitoring_slo.latency[0].name}", "3600s")
        | every 60s
        | condition val() > 14.4 '1'
      EOT
      duration = "0s"
      trigger {
        count = 1
      }
    }
  }

  conditions {
    display_name = "Slow burn (6h ≥ 6x)"
    condition_monitoring_query_language {
      query    = <<-EOT
        select_slo_burn_rate("${google_monitoring_slo.latency[0].name}", "21600s")
        | every 60s
        | condition val() > 6 '1'
      EOT
      duration = "0s"
      trigger {
        count = 1
      }
    }
  }

  documentation {
    content   = "Latency SLO (95% of requests < 500ms over 28d) error budget is burning. Fast burn = page; slow burn = investigate. Defined in deploy/terraform/monitoring.tf."
    mime_type = "text/markdown"
  }

  notification_channels = var.notification_channels
}
