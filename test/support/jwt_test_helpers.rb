module JwtTestHelpers
  SIGNING_KEY = OpenSSL::PKey::RSA.new(2048)
  SIGNING_JWK = JWT::JWK.new(SIGNING_KEY, { use: "sig", alg: "RS256" })

  def stub_jwks!
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL"))
      .to_return(status: 200, body: { keys: [ SIGNING_JWK.export ] }.to_json,
                 headers: { "Content-Type" => "application/json" })
  end

  def issue_token(client_id:, iss: ENV.fetch("GED_OIDC_ISSUER"), exp: 1.hour.from_now.to_i)
    payload = { sub: client_id, iss: iss, exp: exp, iat: Time.now.to_i }
    JWT.encode(payload, SIGNING_KEY, "RS256", kid: SIGNING_JWK.kid)
  end

  def bearer_header(client_id)
    { "Authorization" => "Bearer #{issue_token(client_id: client_id)}" }
  end
end
