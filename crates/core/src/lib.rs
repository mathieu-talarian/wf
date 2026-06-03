//! Shared, framework-agnostic foundations for the workflow server:
//! typed configuration, the RFC 9457 Problem Details model, and AES-256-GCM
//! token sealing. These are direct ports of `apps/server/src/core/*` from the
//! TypeScript service and carry no actix/HTTP dependency.

pub mod config;
pub mod crypto;
pub mod problem;

pub use config::{Config, ConfigError, LogLevel, NodeEnv};
pub use crypto::{CryptoError, Sealed, TokenCipher};
pub use problem::{ProblemDetails, PROBLEM_TYPE_BASE};
