//! Authentication types shared across crates.
//!
//! Port of the pure parts of `core/auth.ts`: the `AuthedUser` projection of a
//! verified Supabase JWT, the `Bearer` header parser, and `AuthError`. The
//! JWKS verification itself (network + jsonwebtoken) lives in the api crate's
//! `auth` module; this stays dependency-light so `wf-db` can take `AuthedUser`
//! in its upsert signature.

use thiserror::Error;

/// A verified, authenticated user, projected from JWT claims (migration plan
/// §7.1). `name`/`avatar_url` come from `user_metadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthedUser {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Error)]
#[error("{0}")]
pub struct AuthError(pub String);

impl AuthError {
    pub fn new(message: impl Into<String>) -> Self {
        AuthError(message.into())
    }
}

/// Extracts the token from an `Authorization: Bearer <jwt>` header. The scheme
/// is matched case-insensitively; a missing/empty header or a non-Bearer scheme
/// (or empty token) fails. Mirrors `parseBearer`, including the exact messages.
pub fn parse_bearer(header: Option<&str>) -> Result<String, AuthError> {
    let header = match header {
        Some(h) if !h.is_empty() => h,
        _ => return Err(AuthError::new("Missing Authorization header")),
    };
    let mut parts = header.splitn(2, ' ');
    let scheme = parts.next().unwrap_or("");
    let token = parts.next().unwrap_or("");
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(AuthError::new("Authorization header must be Bearer <token>"));
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_bearer_case_insensitive() {
        assert_eq!(parse_bearer(Some("Bearer abc.def.ghi")).unwrap(), "abc.def.ghi");
        assert_eq!(parse_bearer(Some("bearer xyz")).unwrap(), "xyz");
        assert_eq!(parse_bearer(Some("BEARER xyz")).unwrap(), "xyz");
    }

    #[test]
    fn rejects_missing_or_malformed() {
        assert_eq!(
            parse_bearer(None).unwrap_err().to_string(),
            "Missing Authorization header"
        );
        assert_eq!(
            parse_bearer(Some("")).unwrap_err().to_string(),
            "Missing Authorization header"
        );
        assert_eq!(
            parse_bearer(Some("Basic abc")).unwrap_err().to_string(),
            "Authorization header must be Bearer <token>"
        );
        assert_eq!(
            parse_bearer(Some("Bearer")).unwrap_err().to_string(),
            "Authorization header must be Bearer <token>"
        );
        assert_eq!(
            parse_bearer(Some("Bearer ")).unwrap_err().to_string(),
            "Authorization header must be Bearer <token>"
        );
    }
}
