//! Microsoft Entra token validation (resource-server role). Signature against the tenant's JWKS, `iss`, `aud`,
//! `exp`/`nbf`, tenant (`tid`) and a usable `oid`. Fails closed: every problem is a 401 without detail.

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use keyzapper_core::office::{parse_oid, EntraIdentity};
use serde::Deserialize;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Only the algorithm Entra uses for access tokens; everything else (including `none`) is rejected.
const ALGORITHM: Algorithm = Algorithm::RS256;
/// Clock skew tolerated for `exp` and `nbf`.
const LEEWAY_SECONDS: u64 = 60;
/// An unknown `kid` triggers a JWKS refresh at most this often (key rotation without a flood of fetches).
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Where the signing keys come from; HTTP in production, a fixed set in tests.
pub trait JwksSource: Send + Sync {
    fn fetch(&self) -> Result<JwkSet, String>;
}

/// `GET <url>` with the system trust store.
pub struct HttpJwks {
    pub url: String,
    pub timeout: Duration,
}

impl JwksSource for HttpJwks {
    fn fetch(&self) -> Result<JwkSet, String> {
        let mut response = keyzapper_core::gateway::agent(self.timeout).get(&self.url).call().map_err(|e| e.to_string())?;
        if response.status().as_u16() != 200 {
            return Err(format!("JWKS endpoint answered HTTP {}", response.status().as_u16()));
        }
        let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("JWKS is not valid JSON: {e}"))
    }
}

/// Fixed key set (tests, offline validation).
pub struct StaticJwks(pub JwkSet);

impl JwksSource for StaticJwks {
    fn fetch(&self) -> Result<JwkSet, String> {
        Ok(self.0.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// No `Authorization: Bearer …` header.
    MissingToken,
    /// Signature, issuer, audience, lifetime, tenant or claims rejected; the text is a category, never a claim value.
    Invalid(&'static str),
    /// Keys could not be fetched; also answered with 401 (the client retries).
    KeysUnavailable,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::MissingToken => f.write_str("missing authorization header"),
            AuthError::Invalid(reason) => write!(f, "invalid token: {reason}"),
            AuthError::KeysUnavailable => f.write_str("signing keys unavailable"),
        }
    }
}

impl std::error::Error for AuthError {}

#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    tid: Option<String>,
    #[serde(default)]
    scp: Option<String>,
}

struct Cache {
    keys: JwkSet,
    fetched_at: Instant,
}

pub struct JwtVerifier {
    issuer: String,
    tenant_id: String,
    audiences: Vec<String>,
    required_scopes: Vec<String>,
    source: Box<dyn JwksSource>,
    ttl: Duration,
    cache: Mutex<Option<Cache>>,
}

impl JwtVerifier {
    /// `issuer` is the tenant's v2 issuer; `audiences` the accepted `aud` values (at least one).
    pub fn new(issuer: String, tenant_id: String, audiences: Vec<String>, required_scopes: Vec<String>, source: Box<dyn JwksSource>, ttl: Duration) -> Self {
        Self { issuer, tenant_id: tenant_id.to_lowercase(), audiences, required_scopes, source, ttl, cache: Mutex::new(None) }
    }

    /// `Authorization` header value → token.
    pub fn bearer(header: Option<&str>) -> Result<&str, AuthError> {
        let value = header.ok_or(AuthError::MissingToken)?.trim();
        let (scheme, token) = value.split_once(char::is_whitespace).ok_or(AuthError::MissingToken)?;
        if !scheme.eq_ignore_ascii_case("bearer") || token.trim().is_empty() {
            return Err(AuthError::MissingToken);
        }
        Ok(token.trim())
    }

    fn key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let stale = cache.as_ref().is_none_or(|c| c.fetched_at.elapsed() > self.ttl);
        let find = |set: &JwkSet| set.keys.iter().find(|k| k.common.key_id.as_deref() == Some(kid)).cloned();
        let mut found = if stale { None } else { cache.as_ref().and_then(|c| find(&c.keys)) };
        if found.is_none() {
            let may_refresh = stale || cache.as_ref().is_none_or(|c| c.fetched_at.elapsed() > MIN_REFRESH_INTERVAL);
            if may_refresh {
                match self.source.fetch() {
                    Ok(keys) => {
                        found = find(&keys);
                        *cache = Some(Cache { keys, fetched_at: Instant::now() });
                    }
                    Err(e) => {
                        tracing::warn!(error = %crate::logging::redact(&e), "jwks fetch failed");
                        if cache.is_none() {
                            return Err(AuthError::KeysUnavailable);
                        }
                    }
                }
            }
        }
        let jwk = found.ok_or(AuthError::Invalid("unknown signing key"))?;
        if !matches!(jwk.algorithm, AlgorithmParameters::RSA(_)) {
            return Err(AuthError::Invalid("unsupported key type"));
        }
        DecodingKey::from_jwk(&jwk).map_err(|_| AuthError::Invalid("unusable signing key"))
    }

    /// Full validation; returns the identity (`oid`, tenant, scopes) and nothing else from the token.
    pub fn verify(&self, token: &str) -> Result<EntraIdentity, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Invalid("malformed header"))?;
        if header.alg != ALGORITHM {
            return Err(AuthError::Invalid("unsupported algorithm"));
        }
        let kid = header.kid.as_deref().filter(|k| !k.is_empty()).ok_or(AuthError::Invalid("missing kid"))?;
        let key = self.key(kid)?;
        let mut validation = Validation::new(ALGORITHM);
        validation.leeway = LEEWAY_SECONDS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&self.audiences);
        let data = decode::<Claims>(token, &key, &validation).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind::*;
            AuthError::Invalid(match e.kind() {
                InvalidSignature => "bad signature",
                ExpiredSignature => "expired",
                ImmatureSignature => "not yet valid",
                InvalidIssuer => "wrong issuer",
                InvalidAudience => "wrong audience",
                MissingRequiredClaim(_) => "missing claim",
                _ => "rejected",
            })
        })?;
        let oid = data.claims.oid.as_deref().and_then(parse_oid).ok_or(AuthError::Invalid("missing oid"))?;
        if let Some(tid) = data.claims.tid.as_deref() {
            if parse_oid(tid).as_deref() != Some(self.tenant_id.as_str()) {
                return Err(AuthError::Invalid("wrong tenant"));
            }
        }
        let scopes: Vec<String> = data.claims.scp.as_deref().unwrap_or_default().split_whitespace().map(str::to_string).collect();
        if self.required_scopes.iter().any(|required| !scopes.iter().any(|s| s == required)) {
            return Err(AuthError::Invalid("missing scope"));
        }
        Ok(EntraIdentity { oid, tenant_id: self.tenant_id.clone(), scopes })
    }
}

#[cfg(test)]
mod tests {
    use super::JwtVerifier;
    use super::AuthError;

    #[test]
    fn bearer_header_is_parsed_strictly() {
        assert_eq!(JwtVerifier::bearer(Some("Bearer abc.def.ghi")), Ok("abc.def.ghi"));
        assert_eq!(JwtVerifier::bearer(Some("bearer  x ")), Ok("x"));
        assert_eq!(JwtVerifier::bearer(Some("Basic dXNlcjpwdw==")), Err(AuthError::MissingToken));
        assert_eq!(JwtVerifier::bearer(Some("Bearer ")), Err(AuthError::MissingToken));
        assert_eq!(JwtVerifier::bearer(None), Err(AuthError::MissingToken));
    }
}
