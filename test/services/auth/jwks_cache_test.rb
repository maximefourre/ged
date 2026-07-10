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
end
