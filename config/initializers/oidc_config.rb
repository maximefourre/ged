# GED_OIDC_* are otherwise only read at request time (Auth::TokenValidator),
# where a missing var 500s every API call instead of failing the boot.
# Development only warns so the admin UI stays usable without an IdP; the
# SECRET_KEY_BASE_DUMMY guard covers the assets:precompile step of the Docker
# build, which boots the app without the runtime environment.
#
# Token binding (GED_OIDC_TOKEN_BINDING) selects the isolation mechanism:
# - "audience" (default): require GED_OIDC_AUDIENCE; TokenValidator verifies aud.
# - "scope": require GED_OIDC_REQUIRED_SCOPE; TokenValidator checks for required scope.
# The app must refuse to boot in production if the binding mode's requirements aren't met.
module OidcConfig
  def self.verify!(env: Rails.env, logger: Rails.logger)
    return if env.test? || ENV["SECRET_KEY_BASE_DUMMY"]

    base_required = %w[GED_OIDC_ISSUER GED_OIDC_JWKS_URL]
    binding = ENV.fetch("GED_OIDC_TOKEN_BINDING", "audience")

    required = base_required.dup
    error_messages = []

    case binding
    when "audience"
      required << "GED_OIDC_AUDIENCE"
    when "scope"
      required << "GED_OIDC_REQUIRED_SCOPE"
    else
      error_messages << "Invalid GED_OIDC_TOKEN_BINDING=#{binding} (must be 'audience' or 'scope')"
    end

    missing = required.reject { |var| ENV[var].present? }
    error_messages << "Missing OIDC config: #{missing.join(', ')}" if missing.present?

    return if error_messages.empty?

    message = error_messages.join("; ") + " — API bearer auth cannot work"
    raise message if env.production?

    logger.warn(message)
  end
end

Rails.application.config.after_initialize { OidcConfig.verify! }
