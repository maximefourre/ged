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

  test "recovers from IdP key rotation by re-fetching JWKS on unknown kid" do
    stub_jwks!
    Auth::JwksCache.instance.key_set # prime with the pre-rotation key set
    rotated_key = OpenSSL::PKey::RSA.new(2048)
    rotated_jwk = JWT::JWK.new(rotated_key, { use: "sig", alg: "RS256" })
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL"))
      .to_return(status: 200, body: { keys: [ rotated_jwk.export ] }.to_json,
                 headers: { "Content-Type" => "application/json" })
    token = JWT.encode({ sub: "erp-app", iss: ENV["GED_OIDC_ISSUER"],
                         exp: 1.hour.from_now.to_i }, rotated_key, "RS256",
                       kid: rotated_jwk.kid)
    result = Auth::TokenValidator.call(token)
    assert_equal "erp-app", result.client_identifier
  end

  test "unknown kid forces at most one JWKS re-fetch per rate-limit window" do
    stub_jwks!
    Auth::JwksCache.instance.key_set # prime the cache (1 request)
    other_key = OpenSSL::PKey::RSA.new(2048)
    2.times do
      token = JWT.encode({ sub: "erp-app", iss: ENV["GED_OIDC_ISSUER"],
                           exp: 1.hour.from_now.to_i }, other_key, "RS256",
                         kid: "garbage")
      assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
    end
    # prime + a single forced refresh; the second garbage kid must not re-fetch
    assert_requested(:get, ENV.fetch("GED_OIDC_JWKS_URL"), times: 2)
  end

  test "accepts any aud when GED_OIDC_AUDIENCE is unset" do
    stub_jwks!
    result = Auth::TokenValidator.call(issue_token(client_id: "erp-app", aud: "some-other-api"))
    assert_equal "erp-app", result.client_identifier
  end

  test "verifies the aud claim when GED_OIDC_AUDIENCE is set" do
    stub_jwks!
    ENV["GED_OIDC_AUDIENCE"] = "https://ged.internal"

    result = Auth::TokenValidator.call(
      issue_token(client_id: "erp-app", aud: "https://ged.internal")
    )
    assert_equal "erp-app", result.client_identifier

    # aud may be an array — accepted if it includes the expected audience
    result = Auth::TokenValidator.call(
      issue_token(client_id: "erp-app", aud: [ "https://ged.internal", "other" ])
    )
    assert_equal "erp-app", result.client_identifier

    [ "https://other-api.internal", nil ].each do |wrong_aud|
      token = issue_token(client_id: "erp-app", aud: wrong_aud)
      assert_raises(Auth::TokenValidator::InvalidToken, "aud=#{wrong_aud.inspect}") do
        Auth::TokenValidator.call(token)
      end
    end
  ensure
    ENV.delete("GED_OIDC_AUDIENCE")
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
