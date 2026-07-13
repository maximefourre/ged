require "test_helper"

class Auth::JwksCacheTest < ActiveSupport::TestCase
  test "serves keys from cache without re-fetching until the TTL expires" do
    stub_jwks!
    3.times { Auth::JwksCache.instance.key_set }
    assert_requested(:get, ENV.fetch("GED_OIDC_JWKS_URL"), times: 1)
  end

  test "backs off instead of re-fetching on every call while the IdP is down" do
    stub_jwks!
    cache = Auth::JwksCache.instance
    cache.key_set # prime the cache (1 request)
    cache.force_expire_for_tests!
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_timeout
    3.times { assert_kind_of JWT::JWK::Set, cache.key_set } # stale keys served
    # prime + a single refresh attempt within the backoff window
    assert_requested(:get, ENV.fetch("GED_OIDC_JWKS_URL"), times: 2)
  end

  test "fails closed without hammering the IdP when keys were never fetched" do
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    3.times do
      assert_raises(Auth::JwksCache::FetchError) { Auth::JwksCache.instance.key_set }
    end
    assert_requested(:get, ENV.fetch("GED_OIDC_JWKS_URL"), times: 1)
  end

  test "health reports a never-fetched cache without triggering a fetch" do
    health = Auth::JwksCache.instance.health
    assert_equal false, health[:fetched]
    assert_nil health[:age_seconds]
    assert_equal false, health[:failing]
    assert_not_requested(:get, ENV.fetch("GED_OIDC_JWKS_URL"))
  end

  test "health reports a healthy cache after a successful fetch" do
    stub_jwks!
    cache = Auth::JwksCache.instance
    cache.key_set
    health = cache.health
    assert_equal true, health[:fetched]
    assert_operator health[:age_seconds], :>=, 0
    assert_equal false, health[:failing]
  end

  test "health flags a cache serving stale keys after a failed refresh" do
    stub_jwks!
    cache = Auth::JwksCache.instance
    cache.key_set
    cache.force_expire_for_tests!
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    cache.key_set # refresh fails; stale keys keep being served
    health = cache.health
    assert_equal true, health[:fetched]
    assert_equal true, health[:failing]
  end
end
