//! A throw-away IdP for the auth tests: serves a JWKS over real HTTP on a
//! local port, counts the requests, and signs tokens with fixed test keys.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};

pub const ISSUER: &str = "https://idp.test";
pub const AUDIENCE: &str = "https://ged.internal";
pub const SIGNING_KID: &str = "test-key-1";
pub const OTHER_KID: &str = "test-key-2";

const SIGNING_PEM: &str = include_str!("../fixtures/auth/signing_key.pem");
const OTHER_PEM: &str = include_str!("../fixtures/auth/other_key.pem");
const SIGNING_N: &str = include_str!("../fixtures/auth/signing_key.n");
const OTHER_N: &str = include_str!("../fixtures/auth/other_key.n");

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub fn jwk(kid: &str, n: &str) -> Value {
    json!({ "kty": "RSA", "use": "sig", "alg": "RS256", "kid": kid, "n": n, "e": "AQAB" })
}

pub fn signing_jwks() -> Value {
    json!({ "keys": [jwk(SIGNING_KID, SIGNING_N)] })
}

pub fn rotated_jwks() -> Value {
    json!({ "keys": [jwk(OTHER_KID, OTHER_N)] })
}

/// Valid claims for `client`, audience mode.
pub fn claims(client: &str) -> Value {
    json!({ "sub": client, "iss": ISSUER, "aud": AUDIENCE, "exp": now() + 3600 })
}

pub fn sign_with(claims: &Value, pem: &str, kid: Option<&str>) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(str::to_owned);
    let key = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("test key");
    jsonwebtoken::encode(&header, claims, &key).expect("sign")
}

/// Signed with the key the IdP publishes at first.
pub fn sign(claims: &Value) -> String {
    sign_with(claims, SIGNING_PEM, Some(SIGNING_KID))
}

/// Signed with the "rotated" key (kid `OTHER_KID`).
pub fn sign_rotated(claims: &Value) -> String {
    sign_with(claims, OTHER_PEM, Some(OTHER_KID))
}

pub fn sign_unknown(claims: &Value, kid: &str) -> String {
    sign_with(claims, OTHER_PEM, Some(kid))
}

pub struct Idp {
    pub url: String,
    state: Arc<IdpState>,
}

struct IdpState {
    hits: AtomicUsize,
    status: AtomicU16,
    body: std::sync::Mutex<Value>,
}

impl Idp {
    pub async fn start() -> Self {
        let state = Arc::new(IdpState {
            hits: AtomicUsize::new(0),
            status: AtomicU16::new(200),
            body: std::sync::Mutex::new(signing_jwks()),
        });
        async fn jwks(State(s): State<Arc<IdpState>>) -> Response {
            s.hits.fetch_add(1, Ordering::SeqCst);
            let status = StatusCode::from_u16(s.status.load(Ordering::SeqCst)).unwrap();
            let body = s.body.lock().unwrap().to_string();
            (status, body).into_response()
        }
        let app = Router::new()
            .route("/jwks", get(jwks))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self { url, state }
    }

    pub fn hits(&self) -> usize {
        self.state.hits.load(Ordering::SeqCst)
    }

    pub fn fail_with(&self, status: u16) {
        self.state.status.store(status, Ordering::SeqCst);
    }

    pub fn recover(&self) {
        self.state.status.store(200, Ordering::SeqCst);
    }

    pub fn publish(&self, jwks: Value) {
        *self.state.body.lock().unwrap() = jwks;
    }
}
