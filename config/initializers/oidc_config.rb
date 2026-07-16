# GED_OIDC_* are otherwise only read at request time (Auth::TokenValidator),
# where a missing var 500s every API call instead of failing the boot.
# Development only warns so the admin UI stays usable without an IdP; the
# SECRET_KEY_BASE_DUMMY guard covers the assets:precompile step of the Docker
# build, which boots the app without the runtime environment.
#
# GED_OIDC_AUDIENCE is required (not optional): TokenValidator fails closed and
# always verifies aud, so the app must refuse to boot without it in production.
module OidcConfig
  REQUIRED_VARS = %w[GED_OIDC_ISSUER GED_OIDC_JWKS_URL GED_OIDC_AUDIENCE].freeze

  def self.verify!(env: Rails.env, logger: Rails.logger)
    return if env.test? || ENV["SECRET_KEY_BASE_DUMMY"]

    missing = REQUIRED_VARS.reject { |var| ENV[var].present? }
    return if missing.empty?

    message = "Missing OIDC config: #{missing.join(', ')} — API bearer auth cannot work"
    raise message if env.production?

    logger.warn(message)
  end
end

Rails.application.config.after_initialize { OidcConfig.verify! }
