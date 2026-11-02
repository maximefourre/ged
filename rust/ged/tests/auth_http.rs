//! `/api/v1/ping` behind bearer auth: the paths that never reach the database
//! (missing and invalid tokens). The registered/unregistered split is in
//! `auth_access.rs`, which has a database.

mod auth_support;

use auth_support::{Idp, claims, sign};
use axum::body::Body;
use axum::http::Request;
use ged::AppState;
use ged::config::Config;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app(idp: &Idp) -> axum::Router {
    let config = Config::from_lookup(&|k| {
        match k {
            "RAILS_ENV" => Some("test"),
            "GED_OIDC_ISSUER" => Some(auth_support::ISSUER),
            "GED_OIDC_AUDIENCE" => Some(auth_support::AUDIENCE),
            _ => None,
        }
        .map(str::to_owned)
        .or_else(|| (k == "GED_OIDC_JWKS_URL").then(|| idp.url.clone()))
    })
    .unwrap();
    ged::router(AppState::without_database(config))
}

async fn get(app: axum::Router, authorization: Option<&str>) -> (u16, String, String) {
    let mut req = Request::get("/api/v1/ping");
    if let Some(value) = authorization {
        req = req.header("Authorization", value);
    }
    let response = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, ct, String::from_utf8_lossy(&body).into_owned())
}

const MISSING: &str = r#"{"error":"missing bearer token"}"#;
const INVALID: &str = r#"{"error":"invalid token"}"#;

#[tokio::test]
async fn no_authorization_header_is_401_missing_bearer_token() {
    let idp = Idp::start().await;
    let (status, ct, body) = get(app(&idp).await, None).await;
    assert_eq!((status, body.as_str()), (401, MISSING));
    assert_eq!(ct, "application/json; charset=utf-8");
    assert_eq!(idp.hits(), 0, "no token, no JWKS fetch");
}

#[tokio::test]
async fn a_scheme_other_than_bearer_is_a_missing_token() {
    let idp = Idp::start().await;
    for header in [
        "Basic abc",
        "bearer abc",
        "Bearer",
        "Bearer ",
        "Bearer    ",
        "Token x",
    ] {
        let (status, _, body) = get(app(&idp).await, Some(header)).await;
        assert_eq!((status, body.as_str()), (401, MISSING), "{header:?}");
    }
}

#[tokio::test]
async fn a_garbage_token_is_401_invalid_token_with_a_fixed_body() {
    let idp = Idp::start().await;
    let (status, _, body) = get(app(&idp).await, Some("Bearer garbage")).await;
    assert_eq!((status, body.as_str()), (401, INVALID));
}

#[tokio::test]
async fn a_token_with_a_hostile_kid_does_not_reflect_it() {
    let idp = Idp::start().await;
    let token = auth_support::sign_unknown(&claims("x"), "../../../etc/passwd");
    let (status, _, body) = get(app(&idp).await, Some(&format!("Bearer {token}"))).await;
    assert_eq!((status, body.as_str()), (401, INVALID));
}

#[tokio::test]
async fn an_unreachable_idp_is_401_not_500() {
    let idp = Idp::start().await;
    idp.fail_with(500);
    let (status, _, body) = get(
        app(&idp).await,
        Some(&format!("Bearer {}", sign(&claims("x")))),
    )
    .await;
    assert_eq!((status, body.as_str()), (401, INVALID));
}

#[tokio::test]
async fn ping_is_unreachable_with_a_verb_it_does_not_serve() {
    let idp = Idp::start().await;
    let req = Request::post("/api/v1/ping").body(Body::empty()).unwrap();
    let response = app(&idp).await.oneshot(req).await.unwrap();
    assert_eq!(response.status().as_u16(), 404);
}
