require "test_helper"

class Auth::TokenValidatorTest < ActiveSupport::TestCase
  test "returns the client identifier from a valid token" do
    stub_jwks!
    result = Auth::TokenValidator.call(issue_token(client_id: "erp-app"))
    assert_equal "erp-app", result.client_identifier
  end

  test "rejects an expired token" do
    stub_jwks!
    token = issue_token(client_id: "erp-app", exp: 1.hour.ago.to_i)
    assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
  end

  test "rejects a token with the wrong issuer" do
    stub_jwks!
    token = issue_token(client_id: "erp-app", iss: "https://evil.test")
    assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
  end

  test "rejects a token signed by an unknown key" do
    stub_jwks!
    other_key = OpenSSL::PKey::RSA.new(2048)
    token = JWT.encode({ sub: "erp-app", iss: ENV["GED_OIDC_ISSUER"],
                         exp: 1.hour.from_now.to_i }, other_key, "RS256", kid: "unknown")
    assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
  end

  test "fails closed when JWKS was never fetched" do
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    assert_raises(Auth::TokenValidator::InvalidToken) do
      Auth::TokenValidator.call(issue_token(client_id: "erp-app"))
    end
  end

  test "keeps serving stale keys when a refresh fails" do
    stub_jwks!
    Auth::JwksCache.instance.key_set  # prime the cache
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    Auth::JwksCache.instance.force_expire_for_tests!
    result = Auth::TokenValidator.call(issue_token(client_id: "erp-app"))
    assert_equal "erp-app", result.client_identifier
  end
end
