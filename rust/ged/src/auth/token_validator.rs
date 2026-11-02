//! Bearer-token validation (port of `Auth::TokenValidator`).
//!
//! Signature and key lookup go through `jsonwebtoken` (RS256 only, so `none`
//! and HS256-with-the-public-key confusion are refused); every claim check is
//! done here, with the exact semantics of the `jwt` 3.2 gem Rails runs:
//! no leeway, `exp`/`nbf` checked only when present, `aud` an intersection.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::Value;

use super::jwks_cache::JwksCache;
use crate::config::{OidcConfig, TokenBinding};

/// The token is not acceptable. The text is for server-side logs only: the
/// API answers a fixed `invalid token` and never echoes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidToken(pub String);

impl fmt::Display for InvalidToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidToken {}

fn invalid(reason: &str) -> InvalidToken {
    InvalidToken(reason.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    pub client_identifier: String,
}

pub struct TokenValidator {
    cache: Arc<JwksCache>,
    config: OidcConfig,
}

impl TokenValidator {
    pub fn new(cache: Arc<JwksCache>, config: OidcConfig) -> Self {
        Self { cache, config }
    }

    pub async fn call(&self, token: &str) -> Result<Validated, InvalidToken> {
        // Configuration first: nothing is verified against a missing setting.
        let issuer = self
            .config
            .issuer
            .as_deref()
            .ok_or_else(|| invalid("GED_OIDC_ISSUER is not set"))?;
        match &self.config.binding {
            TokenBinding::Audience if self.config.audience.is_none() => {
                return Err(invalid("GED_OIDC_AUDIENCE is not set"));
            }
            TokenBinding::Scope if self.config.required_scope.is_none() => {
                return Err(invalid("GED_OIDC_REQUIRED_SCOPE is not set"));
            }
            TokenBinding::Invalid(other) => {
                return Err(InvalidToken(format!(
                    "unknown GED_OIDC_TOKEN_BINDING={other}"
                )));
            }
            _ => {}
        }

        let header = jsonwebtoken::decode_header(token).map_err(|_| invalid("malformed token"))?;
        let kid = header.kid.ok_or_else(|| invalid("no key id (kid)"))?;
        let key = self.decoding_key(&kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims = HashSet::new();
        validation.leeway = 0;
        let claims = jsonwebtoken::decode::<Value>(token, &key, &validation)
            .map_err(|_| invalid("signature verification failed"))?
            .claims;

        check_time(&claims, now())?;
        if claims.get("iss").and_then(Value::as_str) != Some(issuer) {
            return Err(invalid("invalid issuer"));
        }
        match &self.config.binding {
            TokenBinding::Audience => {
                let expected = self.config.audience.as_deref().unwrap_or_default();
                if !audience_matches(&claims, expected) {
                    return Err(invalid("invalid audience"));
                }
            }
            TokenBinding::Scope => {
                let required = self.config.required_scope.as_deref().unwrap_or_default();
                if !token_scopes(&claims).contains(required) {
                    return Err(InvalidToken(format!(
                        "token missing required scope {required}"
                    )));
                }
            }
            TokenBinding::Invalid(_) => return Err(invalid("unknown token binding")),
        }

        let claim = &self.config.client_id_claim;
        let client_identifier = match claims.get(claim) {
            Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err(InvalidToken(format!("token missing {claim} claim"))),
        };
        Ok(Validated { client_identifier })
    }

    /// The key for `kid`; an unknown `kid` forces one (rate-limited) refresh,
    /// which is how IdP key rotation is picked up.
    async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, InvalidToken> {
        let keys = self
            .cache
            .key_set(false)
            .await
            .map_err(|e| InvalidToken(e.0))?;
        let jwk = match find(&keys, kid) {
            Some(jwk) => jwk.clone(),
            None => {
                let keys = self
                    .cache
                    .key_set(true)
                    .await
                    .map_err(|e| InvalidToken(e.0))?;
                find(&keys, kid)
                    .cloned()
                    .ok_or_else(|| invalid("could not find public key for kid"))?
            }
        };
        DecodingKey::from_jwk(&jwk).map_err(|_| invalid("unusable public key"))
    }
}

fn find<'a>(keys: &'a JwkSet, kid: &str) -> Option<&'a jsonwebtoken::jwk::Jwk> {
    keys.keys
        .iter()
        .find(|k| k.common.key_id.as_deref() == Some(kid))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Ruby `value.to_i` on a JSON claim; anything that is not a number or a
/// string counts as `0` (so an `exp` of `null` is long expired).
fn ruby_to_i(value: &Value) -> i64 {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Value::String(s) => {
            let t = s.trim_start();
            let (sign, digits) = match t.strip_prefix('-') {
                Some(rest) => (-1, rest),
                None => (1, t.strip_prefix('+').unwrap_or(t)),
            };
            let end = digits
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(digits.len());
            sign * digits[..end].parse::<i64>().unwrap_or(0)
        }
        _ => 0,
    }
}

/// `exp <= now` is expired and `nbf > now` is premature, both only when the
/// claim is present (as in `jwt` 3.2).
fn check_time(claims: &Value, now: i64) -> Result<(), InvalidToken> {
    if let Some(exp) = claims.get("exp")
        && ruby_to_i(exp) <= now
    {
        return Err(invalid("signature has expired"));
    }
    if let Some(nbf) = claims.get("nbf")
        && ruby_to_i(nbf) > now
    {
        return Err(invalid("signature nbf has not been reached"));
    }
    Ok(())
}

fn audience_matches(claims: &Value, expected: &str) -> bool {
    match claims.get("aud") {
        Some(Value::String(s)) => s == expected,
        Some(Value::Array(list)) => list.iter().any(|v| v.as_str() == Some(expected)),
        _ => false,
    }
}

/// RFC 6749 space-delimited `scope` string plus the `scp` array.
fn token_scopes(claims: &Value) -> HashSet<&str> {
    let mut scopes = HashSet::new();
    if let Some(Value::String(s)) = claims.get("scope") {
        scopes.extend(s.split_whitespace());
    }
    if let Some(Value::Array(list)) = claims.get("scp") {
        scopes.extend(list.iter().filter_map(Value::as_str));
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ruby_to_i_follows_string_to_i() {
        assert_eq!(ruby_to_i(&json!(5)), 5);
        assert_eq!(ruby_to_i(&json!(5.9)), 5);
        assert_eq!(ruby_to_i(&json!("12abc")), 12);
        assert_eq!(ruby_to_i(&json!("abc")), 0);
        assert_eq!(ruby_to_i(&json!(null)), 0);
        assert_eq!(ruby_to_i(&json!("-3")), -3);
    }

    #[test]
    fn exp_and_nbf_boundaries() {
        assert!(check_time(&json!({"exp": 100}), 100).is_err());
        assert!(check_time(&json!({"exp": 101}), 100).is_ok());
        assert!(check_time(&json!({"nbf": 100}), 100).is_ok());
        assert!(check_time(&json!({"nbf": 101}), 100).is_err());
        assert!(check_time(&json!({}), 100).is_ok());
    }
}
