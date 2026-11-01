//! Port of `test/security/auth_matrix_test.rb` (plus the overlapping
//! `test/controllers/api/v1/authentication_test.rb`), as black-box tests.
//!
//! Differences forced by the black-box setting, none of them weakening:
//! - tokens come from the real oauth-provider; the claims the Ruby tests bend
//!   (`exp`, `iss`, `aud`, `nbf`) are re-signed with the provider's own key;
//! - the Ruby test asserted `GED_OIDC_AUDIENCE` was set in its env; here the
//!   foreign-audience token is rejected, which proves it is enforced;
//! - bodies are asserted exactly, including the messages Rails generates.

mod common;

use common::World;
use parity::forge;
use parity::json::{assert_error, assert_json_content_type, assert_json_eq, assert_status};
use serde_json::json;

const PING: &str = "/api/v1/ping";

#[tokio::test]
async fn auth_no_authorization_header_returns_401() {
    let w = World::new().await;
    let r = w.api.get(PING, &[]).await;
    assert_error(&r, 401, "missing bearer token");
}

#[tokio::test]
async fn auth_garbage_bearer_token_returns_401() {
    let w = World::new().await;
    let r = w.api.get_bearer(PING, "garbage").await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_alg_none_forged_token_is_rejected() {
    let w = World::new().await;
    let token = forge::alg_none(&w.template.fresh_claims());
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_hs256_confusion_token_signed_with_the_public_key_is_rejected() {
    let w = World::new().await;
    let token = forge::sign_hs256_with_public_key(&w.template.fresh_claims(), w.template.kid());
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_tampered_payload_is_rejected() {
    let w = World::new().await;
    let mut parts: Vec<String> = w.token.split('.').map(str::to_owned).collect();
    let last = parts[1].pop().expect("payload");
    parts[1].push(if last == 'A' { 'B' } else { 'A' });
    let r = w.api.get_bearer(PING, &parts.join(".")).await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_stripped_signature_is_rejected() {
    let w = World::new().await;
    let mut parts = w.token.split('.');
    let (header, payload) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let r = w
        .api
        .get_bearer(PING, &format!("{header}.{payload}."))
        .await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_expired_token_is_rejected() {
    let w = World::new().await;
    let mut claims = w.template.fresh_claims();
    claims["exp"] = json!(forge::now() - 3600);
    let token = forge::sign_rs256(&claims, w.template.kid());
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_token_with_a_future_nbf_is_rejected() {
    let w = World::new().await;
    let mut claims = w.template.fresh_claims();
    claims["exp"] = json!(forge::now() + 7200);
    claims["nbf"] = json!(forge::now() + 3600);
    let token = forge::sign_rs256(&claims, w.template.kid());
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_wrong_issuer_is_rejected() {
    let w = World::new().await;
    let mut claims = w.template.fresh_claims();
    claims["iss"] = json!("https://evil.test");
    let token = forge::sign_rs256(&claims, w.template.kid());
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 401, "invalid token");
}

#[tokio::test]
async fn auth_valid_token_for_a_registered_client_returns_200() {
    let w = World::new().await;
    let r = w.api.get_bearer(PING, &w.token).await;
    assert_status(&r, 200);
    assert_json_content_type(&r);
    assert_json_eq(&r, &json!({ "client": w.client.client_id }));
}

#[tokio::test]
async fn auth_valid_token_for_an_unregistered_client_returns_403_not_401() {
    // Auth oracle: a cryptographically valid token for a client never
    // registered in GED is a registration failure (403), distinct from every
    // authentication failure above (401). Registered in the provider only.
    let w = World::new().await;
    let ghost = parity::oauth::register(&parity::fixtures::unique("ghost-app"));
    let token = parity::oauth::fetch_token(&ghost).await;
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 403, "client not registered");
}

#[tokio::test]
async fn auth_foreign_or_missing_audience_is_rejected() {
    // GED-SEC-01: aud is verified unconditionally (fail-closed). A token for
    // another internal API, and a token with no aud at all, are both refused.
    let w = World::new().await;
    assert!(
        w.template.claims.get("aud").is_some(),
        "genuine token carries an aud"
    );

    let mut foreign = w.template.fresh_claims();
    foreign["aud"] = json!("https://some-other-api.internal");
    let token = forge::sign_rs256(&foreign, w.template.kid());
    assert_error(&w.api.get_bearer(PING, &token).await, 401, "invalid token");

    let mut audless = w.template.fresh_claims();
    audless
        .as_object_mut()
        .expect("claims object")
        .remove("aud");
    let token = forge::sign_rs256(&audless, w.template.kid());
    assert_error(&w.api.get_bearer(PING, &token).await, 401, "invalid token");
}

#[tokio::test]
async fn auth_failures_return_a_generic_error_body_not_jwt_gem_internals() {
    // GED-SEC-02: the 401 body must be a fixed generic message.
    let w = World::new().await;
    let r = w.api.get_bearer(PING, "garbage").await;
    assert_error(&r, 401, "invalid token");
    assert!(
        !r.text().contains("segments"),
        "leaked internals: {}",
        r.text()
    );
}

#[tokio::test]
async fn auth_a_malicious_kid_is_not_reflected_in_the_error_body() {
    let w = World::new().await;
    let token = forge::sign_rs256(&w.template.fresh_claims(), "../../../etc/passwd");
    let r = w.api.get_bearer(PING, &token).await;
    assert_error(&r, 401, "invalid token");
    assert!(
        !r.text().contains("etc/passwd"),
        "kid reflected: {}",
        r.text()
    );
    assert!(
        !r.text().contains("public key"),
        "internals leaked: {}",
        r.text()
    );
}

// --- from test/controllers/api/v1/authentication_test.rb, not already above ---

#[tokio::test]
async fn auth_non_bearer_scheme_is_treated_as_a_missing_token() {
    // Rails' `\ABearer (.+)\z` match: any other scheme yields no token.
    let w = World::new().await;
    let basic = format!("Basic {}", w.token);
    let r = w.api.get(PING, &[("Authorization", basic.as_str())]).await;
    assert_error(&r, 401, "missing bearer token");
}

#[tokio::test]
async fn auth_control_resigned_untouched_claims_are_accepted() {
    // Control for the forged-token tests above: the forging key and kid are
    // right, so only the bent claim can explain a rejection.
    let w = World::new().await;
    let token = forge::sign_rs256(&w.template.fresh_claims(), w.template.kid());
    let r = w.api.get_bearer(PING, &token).await;
    assert_json_eq(&r, &json!({ "client": w.client.client_id }));
}
