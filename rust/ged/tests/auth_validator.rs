//! Port of `test/services/auth/token_validator_test.rb`, plus the claim
//! edges `jwt` 3.2 decides in Rails (exp/nbf/aud semantics).

mod auth_support;

use std::sync::Arc;

use auth_support::{AUDIENCE, ISSUER, Idp, claims, now, sign, sign_rotated, sign_unknown};
use ged::auth::{InvalidToken, JwksCache, TokenValidator};
use ged::config::{OidcConfig, TokenBinding};
use serde_json::{Value, json};

fn config(binding: TokenBinding) -> OidcConfig {
    OidcConfig {
        issuer: Some(ISSUER.into()),
        jwks_url: None,
        binding,
        audience: Some(AUDIENCE.into()),
        required_scope: Some("urn:test:ged".into()),
        client_id_claim: "sub".into(),
    }
}

struct Fixture {
    idp: Idp,
    cache: Arc<JwksCache>,
}

impl Fixture {
    async fn new() -> Self {
        let idp = Idp::start().await;
        let cache = Arc::new(JwksCache::new(idp.url.clone()));
        Self { idp, cache }
    }

    fn validator(&self, cfg: OidcConfig) -> TokenValidator {
        TokenValidator::new(self.cache.clone(), cfg)
    }

    async fn id(&self, cfg: OidcConfig, token: &str) -> Result<String, InvalidToken> {
        self.validator(cfg)
            .call(token)
            .await
            .map(|r| r.client_identifier)
    }
}

fn audience_cfg() -> OidcConfig {
    config(TokenBinding::Audience)
}

fn scope_cfg() -> OidcConfig {
    config(TokenBinding::Scope)
}

#[tokio::test]
async fn returns_the_client_identifier_from_a_valid_token() {
    let f = Fixture::new().await;
    let got = f.id(audience_cfg(), &sign(&claims("erp-app"))).await;
    assert_eq!(got.unwrap(), "erp-app");
}

#[tokio::test]
async fn rejects_an_expired_token() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["exp"] = json!(now() - 3600);
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err());
}

#[tokio::test]
async fn a_token_expiring_this_very_second_is_expired() {
    // jwt 3.2: `exp <= now` (no leeway) raises ExpiredSignature.
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["exp"] = json!(now());
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err());
}

#[tokio::test]
async fn rejects_a_token_whose_nbf_is_in_the_future_but_not_one_that_just_started() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["nbf"] = json!(now() + 3600);
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err());
    c["nbf"] = json!(now() - 5);
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
}

#[tokio::test]
async fn a_token_without_exp_is_accepted_like_jwt_3_2() {
    // jwt 3.2 only checks exp when present; Rails therefore accepts it.
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c.as_object_mut().unwrap().remove("exp");
    assert_eq!(f.id(audience_cfg(), &sign(&c)).await.unwrap(), "erp-app");
}

#[tokio::test]
async fn exp_given_as_a_string_is_read_like_string_to_i() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["exp"] = json!((now() + 3600).to_string());
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
    c["exp"] = json!(format!("{}abc", now() + 3600));
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
    for expired in [
        json!("abc"),
        json!(""),
        json!(null),
        json!(true),
        json!([now() + 3600]),
        json!({"v": now() + 3600}),
        json!((now() - 1).to_string()),
        json!("-99999999999999999999"),
    ] {
        c["exp"] = expired.clone();
        assert!(
            f.id(audience_cfg(), &sign(&c)).await.is_err(),
            "exp {expired} must be rejected"
        );
    }
}

#[tokio::test]
async fn an_exp_beyond_i64_is_far_future_not_zero() {
    // Ruby bignum: still valid, exactly like Rails.
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["exp"] = json!("99999999999999999999");
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
}

#[tokio::test]
async fn nbf_given_as_a_string_is_read_like_string_to_i() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["nbf"] = json!((now() - 5).to_string());
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
    c["nbf"] = json!("12abc");
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
    c["nbf"] = json!(null); // nil.to_i == 0, long started
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
    c["nbf"] = json!((now() + 3600).to_string());
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err());
}

#[tokio::test]
async fn an_unrepresentable_nbf_is_rejected_never_read_as_zero() {
    // Rails: ImmatureSignature. A fallback to 0 would accept the token.
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    for nbf in [
        json!("99999999999999999999"),
        json!("9_9999999999"),
        json!("9223372036854775808"),
        json!(1e30),
        json!(18446744073709551615u64),
    ] {
        c["nbf"] = nbf.clone();
        assert!(
            f.id(audience_cfg(), &sign(&c)).await.is_err(),
            "nbf {nbf} must be rejected"
        );
    }
}

#[tokio::test]
async fn a_token_whose_nbf_is_this_very_second_is_accepted() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["nbf"] = json!(now());
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
}

#[tokio::test]
async fn rejects_the_wrong_issuer_or_none() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["iss"] = json!("https://evil.test");
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err());
    c.as_object_mut().unwrap().remove("iss");
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err());
}

#[tokio::test]
async fn rejects_a_token_signed_by_an_unknown_key() {
    let f = Fixture::new().await;
    let token = sign_unknown(&claims("erp-app"), "unknown");
    assert!(f.id(audience_cfg(), &token).await.is_err());
}

#[tokio::test]
async fn rejects_a_known_kid_with_a_forged_signature() {
    let f = Fixture::new().await;
    let forged = auth_support::sign_with(
        &claims("erp-app"),
        include_str!("fixtures/auth/other_key.pem"),
        Some(auth_support::SIGNING_KID),
    );
    assert!(f.id(audience_cfg(), &forged).await.is_err());
}

#[tokio::test]
async fn rejects_a_token_without_kid() {
    let f = Fixture::new().await;
    let token = auth_support::sign_with(
        &claims("erp-app"),
        include_str!("fixtures/auth/signing_key.pem"),
        None,
    );
    assert!(f.id(audience_cfg(), &token).await.is_err());
}

#[tokio::test]
async fn rejects_garbage_none_and_hs256_tokens() {
    let f = Fixture::new().await;
    assert!(f.id(audience_cfg(), "garbage").await.is_err());
    // alg none, hand-made.
    let b64 = |v: &Value| {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
    };
    let none = format!(
        "{}.{}.",
        b64(&json!({"alg": "none", "kid": auth_support::SIGNING_KID})),
        b64(&claims("erp-app"))
    );
    assert!(f.id(audience_cfg(), &none).await.is_err());
    // HS256 signed with the JWK modulus as secret (algorithm confusion).
    let hs = jsonwebtoken::encode(
        &{
            let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
            h.kid = Some(auth_support::SIGNING_KID.into());
            h
        },
        &claims("erp-app"),
        &jsonwebtoken::EncodingKey::from_secret(b"anything"),
    )
    .unwrap();
    assert!(f.id(audience_cfg(), &hs).await.is_err());
}

#[tokio::test]
async fn fails_closed_when_jwks_was_never_fetched() {
    let f = Fixture::new().await;
    f.idp.fail_with(500);
    assert!(
        f.id(audience_cfg(), &sign(&claims("erp-app")))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recovers_from_idp_key_rotation_by_refetching_on_unknown_kid() {
    let f = Fixture::new().await;
    f.cache.key_set(false).await.unwrap(); // prime with the pre-rotation set
    f.idp.publish(auth_support::rotated_jwks());
    let got = f
        .id(audience_cfg(), &sign_rotated(&claims("erp-app")))
        .await;
    assert_eq!(got.unwrap(), "erp-app");
}

#[tokio::test]
async fn an_unknown_kid_forces_at_most_one_refetch_per_window() {
    let f = Fixture::new().await;
    f.cache.key_set(false).await.unwrap(); // 1 request
    for _ in 0..2 {
        let token = sign_unknown(&claims("erp-app"), "garbage");
        assert!(f.id(audience_cfg(), &token).await.is_err());
    }
    assert_eq!(f.idp.hits(), 2, "prime + one forced refresh");
}

#[tokio::test]
async fn reads_the_client_identifier_from_the_configured_claim() {
    let f = Fixture::new().await;
    let mut cfg = audience_cfg();
    cfg.client_id_claim = "client_id".into();
    let mut c = claims("ignored");
    c["client_id"] = json!("erp-app");
    assert_eq!(f.id(cfg.clone(), &sign(&c)).await.unwrap(), "erp-app");
    // A token carrying only `sub` is rejected when the custom claim is required.
    assert!(f.id(cfg, &sign(&claims("erp-app"))).await.is_err());
}

#[tokio::test]
async fn a_blank_or_non_scalar_client_claim_is_rejected() {
    let f = Fixture::new().await;
    for bad in [
        json!(""),
        json!("   "),
        json!(null),
        json!(["a"]),
        json!({"a": 1}),
    ] {
        let mut c = claims("x");
        c["sub"] = bad.clone();
        assert!(f.id(audience_cfg(), &sign(&c)).await.is_err(), "{bad}");
    }
}

#[tokio::test]
async fn audience_binding_verifies_aud_string_or_array() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["aud"] = json!([AUDIENCE, "other"]);
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_ok());
    for wrong in [json!("https://other-api.internal"), json!([]), json!(null)] {
        c["aud"] = wrong.clone();
        assert!(f.id(audience_cfg(), &sign(&c)).await.is_err(), "{wrong}");
    }
    c.as_object_mut().unwrap().remove("aud");
    assert!(f.id(audience_cfg(), &sign(&c)).await.is_err(), "no aud");
}

#[tokio::test]
async fn audience_binding_without_a_configured_audience_fails_closed() {
    let f = Fixture::new().await;
    let mut cfg = audience_cfg();
    cfg.audience = None;
    assert!(f.id(cfg, &sign(&claims("erp-app"))).await.is_err());
}

#[tokio::test]
async fn keeps_serving_stale_keys_when_a_refresh_fails() {
    let f = Fixture::new().await;
    f.cache.key_set(false).await.unwrap();
    f.idp.fail_with(500);
    f.cache.force_expire_for_tests();
    let got = f.id(audience_cfg(), &sign(&claims("erp-app"))).await;
    assert_eq!(got.unwrap(), "erp-app");
}

#[tokio::test]
async fn scope_binding_accepts_the_scope_claim_string() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["scope"] = json!("openid urn:test:ged profile");
    assert!(f.id(scope_cfg(), &sign(&c)).await.is_ok());
}

#[tokio::test]
async fn scope_binding_accepts_the_scp_claim_array() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["scp"] = json!(["openid", "urn:test:ged"]);
    assert!(f.id(scope_cfg(), &sign(&c)).await.is_ok());
}

#[tokio::test]
async fn scope_binding_rejects_a_token_missing_the_scope() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["scope"] = json!("openid profile");
    assert!(f.id(scope_cfg(), &sign(&c)).await.is_err());
    assert!(f.id(scope_cfg(), &sign(&claims("erp-app"))).await.is_err());
    // A scope that merely contains the required one as a substring.
    c["scope"] = json!("urn:test:ged-admin");
    assert!(f.id(scope_cfg(), &sign(&c)).await.is_err());
}

#[tokio::test]
async fn scope_binding_does_not_verify_aud() {
    let f = Fixture::new().await;
    let mut c = claims("erp-app");
    c["aud"] = json!("https://some-other-api.internal");
    c["scope"] = json!("urn:test:ged");
    assert!(f.id(scope_cfg(), &sign(&c)).await.is_ok());
}

#[tokio::test]
async fn scope_binding_without_a_configured_scope_fails_closed() {
    let f = Fixture::new().await;
    let mut cfg = scope_cfg();
    cfg.required_scope = None;
    let mut c = claims("erp-app");
    c["scope"] = json!("urn:test:ged");
    assert!(f.id(cfg, &sign(&c)).await.is_err());
}

#[tokio::test]
async fn an_unknown_binding_rejects_everything() {
    let f = Fixture::new().await;
    let cfg = config(TokenBinding::Invalid("both".into()));
    assert!(f.id(cfg, &sign(&claims("erp-app"))).await.is_err());
}

#[tokio::test]
async fn a_missing_issuer_config_rejects_everything() {
    let f = Fixture::new().await;
    let mut cfg = audience_cfg();
    cfg.issuer = None;
    assert!(f.id(cfg, &sign(&claims("erp-app"))).await.is_err());
}

#[tokio::test]
async fn the_rejection_never_echoes_the_token_or_its_kid() {
    let f = Fixture::new().await;
    let token = sign_unknown(&claims("erp-app"), "../../../etc/passwd");
    let err = f.id(audience_cfg(), &token).await.unwrap_err();
    assert!(!err.to_string().contains("etc/passwd"), "{err}");
}
