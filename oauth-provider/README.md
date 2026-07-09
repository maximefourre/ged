# oauth-provider — dev/test OAuth2 stopgap

Stands in for the company IdP until it is accessible. Issues RS256
client_credentials JWTs and exposes JWKS + OIDC discovery. SQLite storage,
secrets stored as SHA-256 digests — dev-only, not a production credential store.

## Usage

    bundle install
    bundle exec rackup -p 9292          # start on :9292
    bin/register-client my-app          # prints client_id + secret (once)

    # get a token
    curl -s -X POST http://localhost:9292/token \
      -d grant_type=client_credentials -d client_id=my-app -d client_secret=<secret>

Cutover to the real IdP = change `GED_OIDC_ISSUER` / `GED_OIDC_JWKS_URL` on the
GED web service, then delete this app.
