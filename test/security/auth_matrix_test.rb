require "test_helper"

# White-box regression suite from the 2026-07-15 pentest pass
# (docs/security/pentest-2026-07-15.md). Locks in the defenses that already
# hold at the bearer-token authentication layer, and documents the one
# confirmed-but-unfixed gap (GED-SEC-01) as a skip so it can be un-skipped
# once fixed.
class AuthMatrixTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @client = AppClient.create!(client_identifier: "erp-app")
  end

  test "no Authorization header returns 401" do
    get "/api/v1/ping"
    assert_response :unauthorized
  end

  test "garbage bearer token returns 401" do
    get "/api/v1/ping", headers: { "Authorization" => "Bearer garbage" }
    assert_response :unauthorized
  end

  test "alg:none forged token is rejected" do
    payload = {
      sub: "erp-app", iss: ENV.fetch("GED_OIDC_ISSUER"),
      exp: 1.hour.from_now.to_i, iat: Time.now.to_i
    }
    token = JWT.encode(payload, nil, "none")
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "HS256 algorithm-confusion token signed with the RSA public key is rejected" do
    payload = {
      sub: "erp-app", iss: ENV.fetch("GED_OIDC_ISSUER"),
      exp: 1.hour.from_now.to_i, iat: Time.now.to_i
    }
    token = JWT.encode(payload, JwtTestHelpers::SIGNING_KEY.public_key.to_pem, "HS256",
                        kid: JwtTestHelpers::SIGNING_JWK.kid)
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "tampered payload (bit flipped after signing) is rejected" do
    token = issue_token(client_id: "erp-app")
    header, payload, signature = token.split(".")
    last = payload[-1]
    flipped = last == "A" ? "B" : "A"
    mutated_payload = payload[0..-2] + flipped
    tampered = [ header, mutated_payload, signature ].join(".")
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{tampered}" }
    assert_response :unauthorized
  end

  test "stripped signature is rejected" do
    token = issue_token(client_id: "erp-app")
    header, payload, = token.split(".")
    unsigned = "#{header}.#{payload}."
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{unsigned}" }
    assert_response :unauthorized
  end

  test "expired token is rejected" do
    token = issue_token(client_id: "erp-app", exp: 1.hour.ago.to_i)
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "token with a future nbf is rejected" do
    payload = {
      sub: "erp-app", iss: ENV.fetch("GED_OIDC_ISSUER"),
      exp: 2.hours.from_now.to_i, iat: Time.now.to_i,
      nbf: 1.hour.from_now.to_i
    }
    token = JWT.encode(payload, JwtTestHelpers::SIGNING_KEY, "RS256",
                        kid: JwtTestHelpers::SIGNING_JWK.kid)
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "wrong issuer is rejected" do
    token = issue_token(client_id: "erp-app", iss: "https://evil.test")
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "valid token for a registered client returns 200" do
    get "/api/v1/ping", headers: bearer_header("erp-app")
    assert_response :success
  end

  test "valid token for an unregistered client returns 403, not 401" do
    # Confirms the auth oracle: a syntactically/cryptographically valid token
    # for a client that was never registered in GED is a registration failure
    # (403), distinct from every authentication failure above (401).
    get "/api/v1/ping", headers: bearer_header("ghost-app")
    assert_response :forbidden
  end

  test "token with a foreign audience is rejected when GED_OIDC_AUDIENCE is set" do
    # GED-SEC-01 (docs/security/pentest-2026-07-15.md), now fixed: aud is
    # verified unconditionally (fail-closed). A token minted for a different
    # internal API (foreign aud), and a token with no aud at all, are both
    # rejected here — a shared company IdP can no longer let one app's token
    # authenticate as another app's GED client.
    assert ENV["GED_OIDC_AUDIENCE"].present?, "test env must set GED_OIDC_AUDIENCE"

    foreign = issue_token(client_id: "erp-app", aud: "https://some-other-api.internal")
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{foreign}" }
    assert_response :unauthorized

    audless = issue_token(client_id: "erp-app", aud: nil)
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{audless}" }
    assert_response :unauthorized
  end
end
