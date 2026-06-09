//! Supabase JWKS token verification (migration plan §7).
//!
//! Port of `core/auth.ts#makeTokenVerifierLive` + the actix extractor that
//! replaces the per-handler `authedUserEffect` prelude. Replicates the verify
//! options exactly: JWKS at `<SUPABASE_URL>/auth/v1/.well-known/jwks.json`,
//! issuer `<SUPABASE_URL>/auth/v1`, audience `SUPABASE_JWT_AUDIENCE`, algorithms
//! ES256/RS256/EdDSA/HS256. Keys are cached by `kid` with a TTL and a forced
//! refetch on a `kid` miss (handles Supabase key rotation), mirroring `jose`'s
//! `createRemoteJWKSet`.

use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use actix_web::dev::Payload;
use actix_web::http::header::AUTHORIZATION;
use actix_web::{web, FromRequest, HttpRequest};
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use wf_core::auth::{parse_bearer, AuthError, AuthedUser};

use crate::error::AppError;
use crate::state::AppState;

const JWKS_TTL: Duration = Duration::from_secs(600);

struct CachedJwks {
    keys: JwkSet,
    fetched_at: Instant,
}

pub struct JwksVerifier {
    jwks_url: String,
    issuer: String,
    audience: String,
    http: reqwest::Client,
    cache: RwLock<Option<CachedJwks>>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    user_metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

impl JwksVerifier {
    pub fn new(supabase_url: &str, audience: &str) -> Self {
        let base = supabase_url.strip_suffix('/').unwrap_or(supabase_url);
        Self {
            jwks_url: format!("{base}/auth/v1/.well-known/jwks.json"),
            issuer: format!("{base}/auth/v1"),
            audience: audience.to_string(),
            http: reqwest::Client::new(),
            cache: RwLock::new(None),
        }
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, AuthError> {
        self.http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::new(format!("JWKS fetch failed: {e}")))?
            .json::<JwkSet>()
            .await
            .map_err(|e| AuthError::new(format!("JWKS parse failed: {e}")))
    }

    /// Returns the JWK for `kid` from cache, refetching on a stale cache or a
    /// `kid` miss (key rotation).
    async fn key_for(&self, kid: &str) -> Result<Jwk, AuthError> {
        if let Some(jwk) = self.cached_key(kid) {
            return Ok(jwk);
        }
        let set = self.fetch_jwks().await?;
        let found = set.find(kid).cloned();
        if let Ok(mut guard) = self.cache.write() {
            *guard = Some(CachedJwks {
                keys: set,
                fetched_at: Instant::now(),
            });
        }
        found.ok_or_else(|| AuthError::new("no matching JWKS key for token kid"))
    }

    fn cached_key(&self, kid: &str) -> Option<Jwk> {
        let guard = self.cache.read().ok()?;
        let cached = guard.as_ref()?;
        if cached.fetched_at.elapsed() >= JWKS_TTL {
            return None;
        }
        cached.keys.find(kid).cloned()
    }

    pub async fn verify(&self, token: &str) -> Result<AuthedUser, AuthError> {
        let header =
            decode_header(token).map_err(|e| AuthError::new(format!("invalid JWT header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| AuthError::new("JWT missing kid"))?;
        let jwk = self.key_for(&kid).await?;
        verify_with_jwk(&jwk, header.alg, token, &self.issuer, &self.audience)
    }
}

/// The signature/claims verification, given a resolved JWK — the pure, testable
/// core of `verify` (the surrounding `key_for` only does HTTP + caching).
fn verify_with_jwk(
    jwk: &Jwk,
    alg: Algorithm,
    token: &str,
    issuer: &str,
    audience: &str,
) -> Result<AuthedUser, AuthError> {
    // Gate to the algorithms the TS verifier accepted; then validate against the
    // token's declared alg (each JWK is single-algorithm, so a mixed
    // `validation.algorithms` is both unnecessary and rejected by jsonwebtoken).
    const ALLOWED: [Algorithm; 4] = [
        Algorithm::ES256,
        Algorithm::RS256,
        Algorithm::EdDSA,
        Algorithm::HS256,
    ];
    if !ALLOWED.contains(&alg) {
        return Err(AuthError::new("unsupported JWT algorithm"));
    }

    let key =
        DecodingKey::from_jwk(jwk).map_err(|e| AuthError::new(format!("invalid JWKS key: {e}")))?;

    let mut validation = Validation::new(alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);

    let data = decode::<Claims>(token, &key, &validation)
        .map_err(|e| AuthError::new(format!("JWT verification failed: {e}")))?;
    if data.claims.sub.is_empty() {
        return Err(AuthError::new("JWT payload schema invalid"));
    }
    Ok(to_authed_user(data.claims))
}

fn meta_str(meta: &Option<serde_json::Map<String, serde_json::Value>>, key: &str) -> Option<String> {
    meta.as_ref()?.get(key)?.as_str().map(String::from)
}

fn to_authed_user(c: Claims) -> AuthedUser {
    AuthedUser {
        id: c.sub,
        email: c.email.unwrap_or_default(),
        name: meta_str(&c.user_metadata, "full_name"),
        avatar_url: meta_str(&c.user_metadata, "avatar_url"),
    }
}

/// Extractor that yields an authenticated user or a 401 problem, replacing the
/// repeated Effect auth prelude. Any handler taking `AuthUser` gets auth-or-401.
pub struct AuthUser(pub AuthedUser);

impl FromRequest for AuthUser {
    type Error = AppError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, AppError>>>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let state = req.app_data::<web::Data<AppState>>().cloned();
        let header = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let instance = req.uri().path_and_query().map(|pq| pq.as_str().to_string());

        Box::pin(async move {
            let state = state.ok_or_else(|| {
                AppError::internal(anyhow::anyhow!("AppState missing")).at(instance.clone())
            })?;
            let token =
                parse_bearer(header.as_deref()).map_err(|e| AppError::from(e).at(instance.clone()))?;
            let user = state
                .jwks
                .verify(&token)
                .await
                .map_err(|e| AppError::from(e).at(instance.clone()))?;
            Ok(AuthUser(user))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    // A throwaway P-256 keypair (generated for this test only) and its public
    // JWK coordinates, so we can sign an ES256 token and verify it end-to-end
    // offline — exercising the exact path used against the live Supabase JWKS.
    const PRIV_PKCS8_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgHivS4ikweo/pmwgC\n\
n6wLX6kRHJkCAiMpEAv73AjyNbWhRANCAASSmFOFEBfaYeiygUYNwyLYTI5kT4Vq\n\
YgnYGs3OkCZ5pruhb0+vxfPGqWkkJDuU+SpMGVTfV7fx1SlDThpEAI7Q\n\
-----END PRIVATE KEY-----";
    const JWK_X: &str = "kphThRAX2mHosoFGDcMi2EyOZE-FamIJ2BrNzpAmeaY";
    const JWK_Y: &str = "u6FvT6_F88apaSQkO5T5KkwZVN9Xt_HVKUNOGkQAjtA";
    const ISSUER: &str = "https://proj.supabase.co/auth/v1";
    const AUDIENCE: &str = "authenticated";

    fn jwk() -> Jwk {
        serde_json::from_value(json!({
            "kty": "EC", "crv": "P-256", "alg": "ES256",
            "use": "sig", "kid": "test-kid", "x": JWK_X, "y": JWK_Y,
        }))
        .unwrap()
    }

    fn sign(claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-kid".to_string());
        let key = EncodingKey::from_ec_pem(PRIV_PKCS8_PEM.as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    fn valid_claims() -> serde_json::Value {
        json!({
            "sub": "11111111-1111-1111-1111-111111111111",
            "email": "user@example.com",
            "user_metadata": { "full_name": "Ada L", "avatar_url": "https://x/a.png" },
            "iss": ISSUER,
            "aud": AUDIENCE,
            "exp": 9_999_999_999i64,
        })
    }

    #[test]
    fn verifies_es256_and_projects_claims() {
        let token = sign(valid_claims());
        let user = verify_with_jwk(&jwk(), Algorithm::ES256, &token, ISSUER, AUDIENCE).unwrap();
        assert_eq!(user.id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(user.email, "user@example.com");
        assert_eq!(user.name.as_deref(), Some("Ada L"));
        assert_eq!(user.avatar_url.as_deref(), Some("https://x/a.png"));
    }

    #[test]
    fn email_defaults_empty_and_metadata_optional() {
        let token = sign(json!({
            "sub": "22222222-2222-2222-2222-222222222222",
            "iss": ISSUER, "aud": AUDIENCE, "exp": 9_999_999_999i64,
        }));
        let user = verify_with_jwk(&jwk(), Algorithm::ES256, &token, ISSUER, AUDIENCE).unwrap();
        assert_eq!(user.email, "");
        assert!(user.name.is_none());
        assert!(user.avatar_url.is_none());
    }

    #[test]
    fn rejects_wrong_issuer_audience_and_expired() {
        let token = sign(valid_claims());
        assert!(verify_with_jwk(&jwk(), Algorithm::ES256, &token, "https://evil/auth/v1", AUDIENCE).is_err());
        assert!(verify_with_jwk(&jwk(), Algorithm::ES256, &token, ISSUER, "wrong-aud").is_err());

        let expired = sign(json!({
            "sub": "x", "iss": ISSUER, "aud": AUDIENCE, "exp": 1_000i64,
        }));
        assert!(verify_with_jwk(&jwk(), Algorithm::ES256, &expired, ISSUER, AUDIENCE).is_err());
    }

    #[test]
    fn rejects_tampered_signature() {
        let mut token = sign(valid_claims());
        // Flip the first char of the signature segment (the last base64url char
        // can carry unused bits and decode identically — not a reliable tamper).
        let sig_start = token.rfind('.').unwrap() + 1;
        let first = token.as_bytes()[sig_start] as char;
        let repl = if first == 'A' { 'B' } else { 'A' };
        token.replace_range(sig_start..sig_start + 1, &repl.to_string());
        assert!(verify_with_jwk(&jwk(), Algorithm::ES256, &token, ISSUER, AUDIENCE).is_err());
    }
}
