module Auth
  class TokenValidator
    class InvalidToken < StandardError; end

    Result = Struct.new(:client_identifier)

    def self.call(token)
      options = {
        algorithms: [ "RS256" ],
        iss: ENV.fetch("GED_OIDC_ISSUER"),
        verify_iss: true,
        verify_expiration: true,
        # Loader form: the jwt gem re-invokes it with kid_not_found: true when
        # the token's kid is absent from the cached set (IdP key rotation).
        jwks: ->(opts) { JwksCache.instance.key_set(force: opts[:kid_not_found]) }
      }
      # Fail-closed: a shared company IdP issues tokens for every internal API
      # under the same iss, so aud must always be enforced. Boot refuses to
      # start without GED_OIDC_AUDIENCE (config/initializers/oidc_config.rb), so
      # ENV.fetch here is safe and a missing/foreign aud raises JWT::InvalidAudError
      # (→ rescued into InvalidToken → 401).
      options[:aud] = ENV.fetch("GED_OIDC_AUDIENCE")
      options[:verify_aud] = true
      payload, = JWT.decode(token, nil, true, options)
      claim = ENV.fetch("GED_CLIENT_ID_CLAIM", "sub")
      identifier = payload[claim]
      raise InvalidToken, "token missing #{claim} claim" if identifier.blank?
      Result.new(identifier)
    rescue JWT::DecodeError, JwksCache::FetchError => e
      raise InvalidToken, e.message
    end
  end
end
