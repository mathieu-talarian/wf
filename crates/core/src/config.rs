//! Typed environment configuration.
//!
//! Port of `core/config.ts` (Effect `Schema`). Same variable names, defaults,
//! and "fail fast at boot" validation intent. Parsing is split from the process
//! environment (`from_map`) so it is unit-testable without mutating global env.

use std::collections::HashMap;

use base64::Engine;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid environment configuration: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeEnv {
    Development,
    Production,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}

impl LogLevel {
    /// Maps the logtape level names to a `tracing` env-filter directive.
    pub fn tracing_directive(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warn",
            LogLevel::Error | LogLevel::Fatal => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub cors_origins: Vec<String>,
    pub node_env: NodeEnv,
    pub log_level: LogLevel,
    pub otel_service_name: String,
    pub otel_exporter_otlp_endpoint: Option<String>,
    pub database_url: String,
    pub supabase_url: String,
    pub supabase_jwt_audience: String,
    pub web_app_url: String,
    pub github_token_encryption_key: String,
}

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_CORS_ORIGIN: &str = "http://localhost:5173";
const DEFAULT_OTEL_SERVICE_NAME: &str = "workflow-server";
const DEFAULT_JWT_AUDIENCE: &str = "authenticated";
const DEFAULT_WEB_APP_URL: &str = "http://localhost:5173";

impl Config {
    /// Loads configuration from the process environment.
    pub fn load() -> Result<Self, ConfigError> {
        let map: HashMap<String, String> = std::env::vars().collect();
        Self::from_map(&map)
    }

    /// Pure configuration parsing from a key/value map. Mirrors the Effect
    /// schema: optional vars get defaults, required vars must be present and
    /// non-empty, `PORT` is a positive integer, `CORS_ORIGINS` is CSV.
    pub fn from_map(map: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let cors_origins = match present(map, "CORS_ORIGINS") {
            None => vec![DEFAULT_CORS_ORIGIN.to_string()],
            Some(raw) => parse_csv(&raw),
        };

        Ok(Config {
            port: parse_port(map)?,
            cors_origins,
            node_env: parse_node_env(map)?,
            log_level: parse_log_level(map)?,
            otel_service_name: present(map, "OTEL_SERVICE_NAME")
                .unwrap_or_else(|| DEFAULT_OTEL_SERVICE_NAME.to_string()),
            otel_exporter_otlp_endpoint: present(map, "OTEL_EXPORTER_OTLP_ENDPOINT"),
            database_url: required(map, "DATABASE_URL")?,
            supabase_url: required(map, "SUPABASE_URL")?,
            supabase_jwt_audience: present(map, "SUPABASE_JWT_AUDIENCE")
                .unwrap_or_else(|| DEFAULT_JWT_AUDIENCE.to_string()),
            web_app_url: present(map, "WEB_APP_URL")
                .unwrap_or_else(|| DEFAULT_WEB_APP_URL.to_string()),
            github_token_encryption_key: required(map, "GITHUB_TOKEN_ENCRYPTION_KEY")?,
        })
    }

    /// Decodes `GITHUB_TOKEN_ENCRYPTION_KEY` to its raw 32 bytes, erroring if it
    /// does not decode to exactly 32 bytes (the TS code's boot-time guard).
    pub fn encryption_key_bytes(&self) -> Result<[u8; 32], ConfigError> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(self.github_token_encryption_key.as_bytes())
            .map_err(|e| ConfigError::Invalid(format!("GITHUB_TOKEN_ENCRYPTION_KEY base64: {e}")))?;
        raw.try_into().map_err(|_| {
            ConfigError::Invalid("GITHUB_TOKEN_ENCRYPTION_KEY must decode to 32 bytes".to_string())
        })
    }
}

/// Parses `PORT`: defaults to [`DEFAULT_PORT`], else must be a positive integer.
fn parse_port(map: &HashMap<String, String>) -> Result<u16, ConfigError> {
    match present(map, "PORT") {
        None => Ok(DEFAULT_PORT),
        Some(raw) => raw
            .parse::<u16>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                ConfigError::Invalid(format!("PORT must be a positive integer, got {raw:?}"))
            }),
    }
}

/// Parses `NODE_ENV`: defaults to `Development`, else one of development|production|test.
fn parse_node_env(map: &HashMap<String, String>) -> Result<NodeEnv, ConfigError> {
    match present(map, "NODE_ENV") {
        None => Ok(NodeEnv::Development),
        Some(raw) => match raw.as_str() {
            "development" => Ok(NodeEnv::Development),
            "production" => Ok(NodeEnv::Production),
            "test" => Ok(NodeEnv::Test),
            other => Err(ConfigError::Invalid(format!(
                "NODE_ENV must be development|production|test, got {other:?}"
            ))),
        },
    }
}

/// Parses `LOG_LEVEL`: defaults to `Info`, else one of the logtape level names.
fn parse_log_level(map: &HashMap<String, String>) -> Result<LogLevel, ConfigError> {
    match present(map, "LOG_LEVEL") {
        None => Ok(LogLevel::Info),
        Some(raw) => match raw.as_str() {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warning" => Ok(LogLevel::Warning),
            "error" => Ok(LogLevel::Error),
            "fatal" => Ok(LogLevel::Fatal),
            other => Err(ConfigError::Invalid(format!(
                "LOG_LEVEL must be one of trace|debug|info|warning|error|fatal, got {other:?}"
            ))),
        },
    }
}

/// Returns a trimmed-presence value: `None` when missing or empty after trim.
/// (Effect's `nonEmptyString`/`optional` treat empty env vars as absent.)
fn present(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key).map(|s| s.to_string()).filter(|s| !s.is_empty())
}

fn required(map: &HashMap<String, String>, key: &str) -> Result<String, ConfigError> {
    present(map, key)
        .ok_or_else(|| ConfigError::Invalid(format!("{key} is required and must be non-empty")))
}

/// CSV → trimmed, non-empty segments. Matches the `CsvStringArray` transform.
fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("DATABASE_URL".into(), "postgres://x".into());
        m.insert("SUPABASE_URL".into(), "https://proj.supabase.co".into());
        // 32 zero bytes, base64-encoded.
        m.insert("GITHUB_TOKEN_ENCRYPTION_KEY".into(), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into());
        m
    }

    #[test]
    fn applies_defaults() {
        let c = Config::from_map(&base()).unwrap();
        assert_eq!(c.port, 3000);
        assert_eq!(c.cors_origins, vec!["http://localhost:5173".to_string()]);
        assert_eq!(c.node_env, NodeEnv::Development);
        assert_eq!(c.log_level, LogLevel::Info);
        assert_eq!(c.otel_service_name, "workflow-server");
        assert_eq!(c.supabase_jwt_audience, "authenticated");
        assert!(c.otel_exporter_otlp_endpoint.is_none());
    }

    #[test]
    fn parses_csv_origins_trimming_and_filtering_empties() {
        let mut m = base();
        m.insert("CORS_ORIGINS".into(), " a , ,b,c ,".into());
        let c = Config::from_map(&m).unwrap();
        assert_eq!(c.cors_origins, vec!["a", "b", "c"]);
    }

    #[test]
    fn missing_required_errors() {
        let mut m = base();
        m.remove("DATABASE_URL");
        assert!(Config::from_map(&m).is_err());
    }

    #[test]
    fn empty_required_treated_as_missing() {
        let mut m = base();
        m.insert("SUPABASE_URL".into(), "".into());
        assert!(Config::from_map(&m).is_err());
    }

    #[test]
    fn rejects_non_positive_or_invalid_port() {
        for bad in ["0", "-1", "abc", "99999999"] {
            let mut m = base();
            m.insert("PORT".into(), bad.into());
            assert!(Config::from_map(&m).is_err(), "expected {bad} to fail");
        }
    }

    #[test]
    fn encryption_key_must_be_32_bytes() {
        let c = Config::from_map(&base()).unwrap();
        assert_eq!(c.encryption_key_bytes().unwrap().len(), 32);

        let mut m = base();
        m.insert("GITHUB_TOKEN_ENCRYPTION_KEY".into(), "c2hvcnQ=".into()); // "short"
        let c = Config::from_map(&m).unwrap();
        assert!(c.encryption_key_bytes().is_err());
    }
}
