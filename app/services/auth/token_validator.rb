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
      # Fail-closed token binding: isolate GED from other services in a shared IdP.
      # Two modes (GED_OIDC_TOKEN_BINDING env var):
      # - "audience" (default): require token.aud == GED_OIDC_AUDIENCE. Safe when
      #   the IdP issues separate audiences per service. Boot requires
      #   GED_OIDC_AUDIENCE in production (config/initializers/oidc_config.rb).
      # - "scope": require token.scope contains GED_OIDC_REQUIRED_SCOPE. Needed
      #   when the IdP issues a shared audience for all services; scope isolation
      #   is the only per-service distinction. Boot requires GED_OIDC_REQUIRED_SCOPE
      #   in production.
      binding = ENV.fetch("GED_OIDC_TOKEN_BINDING", "audience")
      case binding
      when "audience"
        options[:aud] = ENV.fetch("GED_OIDC_AUDIENCE")
        options[:verify_aud] = true
      when "scope"
        # Do not verify aud; will check required scope after decode.
      else
        raise InvalidToken, "unknown GED_OIDC_TOKEN_BINDING=#{binding}"
      end

      payload, = JWT.decode(token, nil, true, options)

      if binding == "scope"
        required_scope = ENV.fetch("GED_OIDC_REQUIRED_SCOPE")
        token_scopes = token_scopes(payload)
        raise InvalidToken, "token missing required scope #{required_scope}" unless token_scopes.include?(required_scope)
      end

      claim = ENV.fetch("GED_CLIENT_ID_CLAIM", "sub")
      identifier = payload[claim]
      raise InvalidToken, "token missing #{claim} claim" if identifier.blank?
      Result.new(identifier)
    rescue JWT::DecodeError, JwksCache::FetchError => e
      raise InvalidToken, e.message
    end

    private_class_method def self.token_scopes(payload)
      scopes = Set.new

      # RFC 6749 / RFC 9068: space-delimited scope claim
      scope_claim = payload["scope"]
      if scope_claim.is_a?(String) && scope_claim.present?
        scopes.merge(scope_claim.split)
      end

      # scp claim: JSON array of scope strings
      scp_claim = payload["scp"]
      if scp_claim.is_a?(Array)
        scopes.merge(scp_claim)
      end

      scopes
    end
  end
end
