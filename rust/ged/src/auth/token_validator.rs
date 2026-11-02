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

/// Ruby `value.to_i` on a JSON claim, saturating where Ruby would return a
/// bignum (so `"99999999999999999999"` is "very far in the future", exactly
/// how Rails judges it) — never a fallback to `0`. Anything that is neither a
/// number nor a string counts as `0` (an `exp` of `null` is long expired).
fn ruby_to_i(value: &Value) -> i64 {
    match value {
        Value::Number(n) => n
            .as_i64()
            // u64 above i64::MAX has no i64 form; as_f64 saturates below.
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Value::String(s) => string_to_i(s),
        _ => 0,
    }
}

/// `String#to_i` (base 10): leading ASCII whitespace, one sign, an optional
/// `0d` prefix, then digits where a single `_` between digits is allowed.
fn string_to_i(s: &str) -> i64 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (negative, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let b = rest.as_bytes();
    let b = match b {
        [b'0', b'd' | b'D', d, ..] if d.is_ascii_digit() => &b[2..],
        _ => b,
    };
    let mut magnitude: u64 = 0;
    let mut prev_digit = false;
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_digit() {
            magnitude = magnitude
                .saturating_mul(10)
                .saturating_add(u64::from(c - b'0'));
            prev_digit = true;
        } else if c == b'_' && prev_digit && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            prev_digit = false;
        } else {
            break;
        }
    }
    if negative {
        i64::try_from(magnitude).map_or(i64::MIN, |m| -m)
    } else {
        i64::try_from(magnitude).unwrap_or(i64::MAX)
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
    fn ruby_to_i_saturates_instead_of_falling_back_to_zero() {
        // Ruby has bignums: these are huge, never 0.
        assert_eq!(ruby_to_i(&json!("99999999999999999999")), i64::MAX);
        assert_eq!(ruby_to_i(&json!("-99999999999999999999")), i64::MIN);
        assert_eq!(ruby_to_i(&json!(1e30)), i64::MAX);
        assert_eq!(ruby_to_i(&json!(99999999999999999999u128 as f64)), i64::MAX);
        assert_eq!(ruby_to_i(&json!(18446744073709551615u64)), i64::MAX);
    }

    #[test]
    fn ruby_to_i_matches_string_to_i_quirks_probed_on_ruby_3_4() {
        assert_eq!(ruby_to_i(&json!("9_9999999999")), 99_999_999_999);
        assert_eq!(ruby_to_i(&json!("1_000")), 1000);
        assert_eq!(ruby_to_i(&json!("1__0")), 1);
        assert_eq!(ruby_to_i(&json!("_1")), 0);
        assert_eq!(ruby_to_i(&json!("1_")), 1);
        assert_eq!(ruby_to_i(&json!("0_8")), 8);
        assert_eq!(ruby_to_i(&json!("0d12")), 12);
        assert_eq!(ruby_to_i(&json!("-0d5")), -5);
        assert_eq!(ruby_to_i(&json!("0d")), 0);
        assert_eq!(ruby_to_i(&json!("0d_1")), 0);
        assert_eq!(ruby_to_i(&json!("0x1A")), 0);
        assert_eq!(ruby_to_i(&json!("1e3")), 1);
        assert_eq!(ruby_to_i(&json!("\u{b}\u{c} 7x")), 7);
        assert_eq!(ruby_to_i(&json!("\u{a0}5")), 0);
        assert_eq!(ruby_to_i(&json!("- 3")), 0);
        assert_eq!(ruby_to_i(&json!("+-3")), 0);
    }

    #[test]
    fn unrepresentable_times_are_judged_like_ruby_bignums() {
        let now = 1_700_000_000;
        assert!(check_time(&json!({"nbf": "99999999999999999999"}), now).is_err());
        assert!(check_time(&json!({"nbf": "9_9999999999"}), now).is_err());
        assert!(check_time(&json!({"nbf": 1e30}), now).is_err());
        assert!(check_time(&json!({"nbf": "-99999999999999999999"}), now).is_ok());
        assert!(check_time(&json!({"exp": "99999999999999999999"}), now).is_ok());
        assert!(check_time(&json!({"exp": "-99999999999999999999"}), now).is_err());
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
