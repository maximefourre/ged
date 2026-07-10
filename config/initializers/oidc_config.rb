# GED_OIDC_* are otherwise only read at request time (Auth::TokenValidator),
# where a missing var 500s every API call instead of failing the boot.
# Development only warns so the admin UI stays usable without an IdP; the
# SECRET_KEY_BASE_DUMMY guard covers the assets:precompile step of the Docker
# build, which boots the app without the runtime environment.
Rails.application.config.after_initialize do
  next if Rails.env.test? || ENV["SECRET_KEY_BASE_DUMMY"]

  missing = %w[GED_OIDC_ISSUER GED_OIDC_JWKS_URL].reject { |var| ENV[var].present? }
  next if missing.empty?

  message = "Missing OIDC config: #{missing.join(', ')} — API bearer auth cannot work"
  raise message if Rails.env.production?

  Rails.logger.warn(message)
end
