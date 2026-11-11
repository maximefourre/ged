# Playwright against the Rust admin UI

The suite talks only to `GED_BASE_URL`. It does not start a server. The
compose service `web` is still Rails, so a run against Rust is a `ged serve`
process beside that stack.

## What must already be up

From the repository root:

```sh
bin/worktree-stack up
```

That publishes Postgres, the temporary OAuth provider, and the Rails `web`
container. Playwright does not use the Rails port. It needs Postgres and the
provider's JWKS URL, because `ged serve` refuses to boot in production
without OIDC settings.

Give Rust its own database on that Postgres. `ged_production` already holds
the Rails schema; `ged migrate` must not be pointed at it.

## Server

Run these from the repository root so uploads land in `./storage` (the same
directory the status page checks). Take `SECRET_KEY_BASE` and
`GED_ADMIN_PASSWORD` from the worktree `.env`. `bin/worktree-stack urls`
prints the Postgres and OAuth host ports (`db`, `oauth`).

```sh
export PGPASSWORD=postgres
psql -h 127.0.0.1 -p "${DB_PORT}" -U postgres -c \
  "DROP DATABASE IF EXISTS ged_playwright WITH (FORCE)"
psql -h 127.0.0.1 -p "${DB_PORT}" -U postgres -c \
  "CREATE DATABASE ged_playwright"

export RAILS_ENV=production
export DATABASE_URL="postgres://postgres:postgres@127.0.0.1:${DB_PORT}/ged_playwright"
export SECRET_KEY_BASE="$(the value from .env)"
export GED_OIDC_ISSUER=http://oauth-provider:9292
export GED_OIDC_JWKS_URL="http://127.0.0.1:${OAUTH_PORT}/.well-known/jwks.json"
export GED_OIDC_AUDIENCE=http://web:80
export GED_ADMIN_USERNAME=admin
export GED_ADMIN_PASSWORD="$(the value from .env)"
export PORT=3099

cargo run -q -p ged --manifest-path rust/Cargo.toml -- migrate
cargo run -q -p ged --manifest-path rust/Cargo.toml -- seed
cargo run -q -p ged --manifest-path rust/Cargo.toml -- serve
# optional, for the status page's worker row:
cargo run -q -p ged --manifest-path rust/Cargo.toml -- worker
```

`ged seed` writes an argon2 hash. That is what the Rust login checks. Do not
run it against `ged_production`: Rails' Devise login checks bcrypt, so the
compose `web` container would reject the admin until `bin/worktree-stack seed`.

## Suite

```sh
cd rust/e2e
npm install
GED_BASE_URL="http://127.0.0.1:${PORT}" \
  GED_ADMIN_USERNAME=admin \
  GED_ADMIN_PASSWORD="$(the value from .env)" \
  npx playwright test
```

`GED_ADMIN_USERNAME` defaults to `admin` inside the suite when it is unset.
`GED_BASE_URL` and `GED_ADMIN_PASSWORD` are required. Chromium is headless
and closed in `fixtures.ts` (`finally`). One worker, no retries.
