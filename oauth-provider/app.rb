require "sinatra/base"
require "json"
require "securerandom"
require_relative "lib/key_store"
require_relative "lib/client_store"

module OauthProvider
  class App < Sinatra::Base
    TOKEN_TTL = 3600

    def issuer
      ENV.fetch("OAUTH_ISSUER", "http://localhost:9292")
    end

    post "/token" do
      content_type :json
      unless params["grant_type"] == "client_credentials"
        halt 400, { error: "unsupported_grant_type" }.to_json
      end
      unless ClientStore.authenticate(params["client_id"], params["client_secret"])
        halt 401, { error: "invalid_client" }.to_json
      end

      now = Time.now.to_i
      payload = {
        iss: issuer,
        sub: params["client_id"],
        iat: now,
        exp: now + TOKEN_TTL,
        jti: SecureRandom.uuid
      }
      token = JWT.encode(payload, KeyStore.signing_key, "RS256", kid: KeyStore.jwk.kid)
      { access_token: token, token_type: "Bearer", expires_in: TOKEN_TTL }.to_json
    end

    get "/.well-known/jwks.json" do
      content_type :json
      KeyStore.jwks.to_json
    end

    get "/.well-known/openid-configuration" do
      content_type :json
      {
        issuer: issuer,
        token_endpoint: "#{issuer}/token",
        jwks_uri: "#{issuer}/.well-known/jwks.json"
      }.to_json
    end
  end
end
