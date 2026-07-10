module Auth
  class TokenValidator
    class InvalidToken < StandardError; end

    Result = Struct.new(:client_identifier)

    def self.call(token)
      payload, = JWT.decode(
        token, nil, true,
        algorithms: [ "RS256" ],
        iss: ENV.fetch("GED_OIDC_ISSUER"),
        verify_iss: true,
        verify_expiration: true,
        # Loader form: the jwt gem re-invokes it with kid_not_found: true when
        # the token's kid is absent from the cached set (IdP key rotation).
        jwks: ->(options) { JwksCache.instance.key_set(force: options[:kid_not_found]) }
      )
      claim = ENV.fetch("GED_CLIENT_ID_CLAIM", "sub")
      identifier = payload[claim]
      raise InvalidToken, "token missing #{claim} claim" if identifier.blank?
      Result.new(identifier)
    rescue JWT::DecodeError, JwksCache::FetchError => e
      raise InvalidToken, e.message
    end
  end
end
