//! Port of `test/services/auth/jwks_cache_test.rb`.

mod auth_support;

use auth_support::Idp;
use ged::auth::JwksCache;

#[tokio::test]
async fn serves_keys_from_cache_without_refetching_until_the_ttl_expires() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    for _ in 0..3 {
        cache.key_set(false).await.expect("keys");
    }
    assert_eq!(idp.hits(), 1);
}

#[tokio::test]
async fn refetches_once_the_ttl_has_elapsed() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    cache.key_set(false).await.expect("keys");
    cache.force_expire_for_tests();
    cache.key_set(false).await.expect("keys");
    assert_eq!(idp.hits(), 2);
}

#[tokio::test]
async fn backs_off_instead_of_refetching_on_every_call_while_the_idp_is_down() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    cache.key_set(false).await.expect("prime");
    cache.force_expire_for_tests();
    idp.fail_with(500);
    for _ in 0..3 {
        cache.key_set(false).await.expect("stale keys are served");
    }
    // the prime plus a single refresh attempt inside the backoff window
    assert_eq!(idp.hits(), 2);
}

#[tokio::test]
async fn fails_closed_without_hammering_the_idp_when_keys_were_never_fetched() {
    let idp = Idp::start().await;
    idp.fail_with(500);
    let cache = JwksCache::new(idp.url.clone());
    for _ in 0..3 {
        assert!(cache.key_set(false).await.is_err());
    }
    assert_eq!(idp.hits(), 1);
}

#[tokio::test]
async fn an_unreachable_idp_fails_closed() {
    let cache = JwksCache::new("http://127.0.0.1:9/jwks".into());
    assert!(cache.key_set(false).await.is_err());
}

#[tokio::test]
async fn a_body_that_is_not_a_key_set_is_a_failed_fetch() {
    let idp = Idp::start().await;
    idp.publish(serde_json::json!({ "nope": 1 }));
    let cache = JwksCache::new(idp.url.clone());
    assert!(cache.key_set(false).await.is_err());
    assert!(!cache.health().fetched);
}

#[tokio::test]
async fn health_reports_a_never_fetched_cache_without_triggering_a_fetch() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    let health = cache.health();
    assert!(!health.fetched);
    assert_eq!(health.age_seconds, None);
    assert!(!health.failing);
    assert_eq!(idp.hits(), 0, "health must not do HTTP");
}

#[tokio::test]
async fn health_reports_a_healthy_cache_after_a_successful_fetch() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    cache.key_set(false).await.expect("keys");
    let health = cache.health();
    assert!(health.fetched);
    assert!(health.age_seconds.is_some_and(|a| a >= 0.0));
    assert!(!health.failing);
}

#[tokio::test]
async fn health_flags_a_cache_serving_stale_keys_after_a_failed_refresh() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    cache.key_set(false).await.expect("keys");
    cache.force_expire_for_tests();
    idp.fail_with(500);
    cache.key_set(false).await.expect("stale");
    let health = cache.health();
    assert!(health.fetched);
    assert!(health.failing);
    assert_eq!(idp.hits(), 2, "health did not fetch either");
}

#[tokio::test]
async fn a_successful_refresh_clears_the_failing_flag() {
    let idp = Idp::start().await;
    let cache = JwksCache::new(idp.url.clone());
    cache.key_set(false).await.expect("keys");
    idp.fail_with(500);
    cache.force_expire_for_tests();
    cache.key_set(false).await.expect("stale");
    assert!(cache.health().failing);
    idp.recover();
    cache.force_expire_for_tests();
    cache.expire_backoff_for_tests();
    cache.key_set(false).await.expect("fresh");
    assert!(!cache.health().failing);
}

#[tokio::test]
async fn concurrent_callers_on_a_cold_cache_share_one_fetch() {
    let idp = Idp::start().await;
    let cache = std::sync::Arc::new(JwksCache::new(idp.url.clone()));
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let cache = cache.clone();
            tokio::spawn(async move { cache.key_set(false).await.is_ok() })
        })
        .collect();
    for t in tasks {
        assert!(t.await.unwrap());
    }
    assert_eq!(idp.hits(), 1);
}
