//! Forged and tampered JWTs for the auth matrix.
//!
//! The suite is black-box, so claims it needs to bend (`exp`, `iss`, `aud`,
//! `nbf`) are signed with the oauth-provider's own signing key, read once from
//! its data volume. Header and base claims come from a genuine token, so the
//! `kid`, issuer and audience always match whatever the stack is configured
//! with.

use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};

use crate::stack;

/// Header and claims of a genuine token, decoded without verification.
#[derive(Debug, Clone)]
pub struct Template {
    pub header: Value,
    pub claims: Value,
}

impl Template {
    pub fn from_token(token: &str) -> Self {
        let mut parts = token.split('.');
        let mut part = || -> Value {
            let raw = parts.next().expect("JWT segment");
            serde_json::from_slice(&B64.decode(raw).expect("base64url segment"))
                .expect("JSON segment")
        };
        let header = part();
        let claims = part();
        Self { header, claims }
    }

    pub fn kid(&self) -> &str {
        self.header["kid"]
            .as_str()
            .expect("genuine token has a kid")
    }

    /// Claims of the genuine token with `exp` pushed to `now + 1h`.
    pub fn fresh_claims(&self) -> Value {
        let mut c = self.claims.clone();
        c["iat"] = json!(now());
        c["exp"] = json!(now() + 3600);
        c
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

/// The provider's RSA private key (PEM), read once.
pub fn signing_key_pem() -> &'static str {
    static PEM: OnceLock<String> = OnceLock::new();
    PEM.get_or_init(|| {
        stack::exec("oauth-provider", &["cat", "data/signing_key.pem"])
            .unwrap_or_else(|e| panic!("read signing key: {e}"))
    })
}

/// The matching public key as SubjectPublicKeyInfo PEM (what Ruby's
/// `public_key.to_pem` yields), used for the HS256 confusion attack.
pub fn public_key_pem() -> &'static str {
    static PEM: OnceLock<String> = OnceLock::new();
    PEM.get_or_init(|| {
        let script = "require 'openssl'; \
                      print OpenSSL::PKey::RSA.new(File.read('data/signing_key.pem')).public_key.to_pem";
        stack::exec("oauth-provider", &["ruby", "-e", script])
            .unwrap_or_else(|e| panic!("derive public key: {e}"))
    })
}

/// Sign `claims` with the provider key, RS256, under `kid`.
pub fn sign_rs256(claims: &Value, kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_owned());
    let key = EncodingKey::from_rsa_pem(signing_key_pem().as_bytes()).expect("RSA key");
    encode(&header, claims, &key).expect("sign RS256")
}

/// HS256 token keyed with the RSA *public* key text (algorithm confusion).
pub fn sign_hs256_with_public_key(claims: &Value, kid: &str) -> String {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(kid.to_owned());
    let key = EncodingKey::from_secret(public_key_pem().as_bytes());
    encode(&header, claims, &key).expect("sign HS256")
}

/// Unsigned `alg: none` token (`<header>.<payload>.`).
pub fn alg_none(claims: &Value) -> String {
    let header = B64.encode(json!({ "alg": "none", "typ": "JWT" }).to_string());
    let payload = B64.encode(claims.to_string());
    format!("{header}.{payload}.")
}
