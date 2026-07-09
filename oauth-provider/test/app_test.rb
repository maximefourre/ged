ENV["APP_ENV"] = "test"
ENV["OAUTH_ISSUER"] = "http://oauth-provider.test"
ENV["OAUTH_DATA_DIR"] = File.expand_path("../tmp_test_data", __dir__)

require "fileutils"
FileUtils.rm_rf(ENV["OAUTH_DATA_DIR"])

require "minitest/autorun"
require "rack/test"
require_relative "../app"

class AppTest < Minitest::Test
  include Rack::Test::Methods

  def app
    OauthProvider::App
  end

  def setup
    @secret = OauthProvider::ClientStore.register("erp-app")
  end

  def teardown
    OauthProvider::ClientStore.reset_for_tests!
  end

  def test_issues_a_valid_jwt_for_correct_credentials
    post "/token", grant_type: "client_credentials",
                   client_id: "erp-app", client_secret: @secret
    assert_equal 200, last_response.status
    body = JSON.parse(last_response.body)
    assert_equal "Bearer", body["token_type"]
    assert_equal 3600, body["expires_in"]

    jwks = JWT::JWK::Set.new(OauthProvider::KeyStore.jwks)
    payload, header = JWT.decode(body["access_token"], nil, true,
                                 algorithms: [ "RS256" ], jwks: jwks)
    assert_equal "erp-app", payload["sub"]
    assert_equal "http://oauth-provider.test", payload["iss"]
    assert payload["exp"] > Time.now.to_i
    assert header["kid"]
  end

  def test_rejects_wrong_secret
    post "/token", grant_type: "client_credentials",
                   client_id: "erp-app", client_secret: "wrong"
    assert_equal 401, last_response.status
  end

  def test_rejects_unknown_client
    post "/token", grant_type: "client_credentials",
                   client_id: "ghost", client_secret: "whatever"
    assert_equal 401, last_response.status
  end

  def test_rejects_unsupported_grant_type
    post "/token", grant_type: "password",
                   client_id: "erp-app", client_secret: @secret
    assert_equal 400, last_response.status
  end

  def test_serves_jwks
    get "/.well-known/jwks.json"
    assert_equal 200, last_response.status
    keys = JSON.parse(last_response.body)["keys"]
    assert_equal 1, keys.size
    assert_equal "RS256", keys.first["alg"]
    refute keys.first.key?("d"), "JWKS must not expose the private key"
  end

  def test_serves_openid_configuration
    get "/.well-known/openid-configuration"
    body = JSON.parse(last_response.body)
    assert_equal "http://oauth-provider.test", body["issuer"]
    assert body["token_endpoint"].end_with?("/token")
    assert body["jwks_uri"].end_with?("/.well-known/jwks.json")
  end
end
