# GED — Document Management System

Self-hosted document store for a 10+ person team/company, organized around
departments (folders, the permission boundary) and cross-cutting tags. Accessed
mostly by other internal applications over an API secured with OAuth2/JWT bearer
tokens, plus a minimal admin web UI for humans.

Full design: `docs/superpowers/specs/2026-07-08-ged-design.md`. Read it before
making architectural changes — this file is a quick-reference summary, the spec
is the source of truth.

## Stack

- **Ruby on Rails** — single app, API-only controllers under `/api/v1/...` for
  application clients, regular controllers + ERB views for the admin UI
- **PostgreSQL** — app data (departments, tags, documents, app_clients, admins)
- **Active Storage** (local disk service) — document file storage
- **Devise** — admin UI username/password session login
- **`jwt` gem** — validates bearer tokens from app clients against a JWKS endpoint
- **Docker Compose** — `web` (Rails/Puma), `postgres`, `oauth-provider`

## Repo layout

- `docs/superpowers/specs/` — design specs (source of truth for architecture
  decisions)
- `docs/superpowers/plans/` — implementation plans (checkbox progress tracking,
  designed for resuming and/or delegation to subagents)
- `oauth-provider/` — temporary standalone OAuth2 dev provider (own Gemfile)

## Key architectural decisions

- **Permission model:** departments are folders and the permission boundary.
  Access to a department covers its whole subtree — no finer-grained
  per-subfolder permissions.
- **Two auth paths:** application clients use OAuth2 client_credentials JWTs
  (validated as a resource server via JWKS); admins use a separate built-in
  username/password login, unrelated to the OIDC flow.
- **Client provisioning is manual:** app clients are registered by an admin and
  explicitly mapped to department access — never auto-provisioned from JWT
  claims, since the real IdP's claim structure isn't known yet.
- **Local OAuth2 provider is temporary:** the company's real IdP isn't
  accessible yet, so a separate small Sinatra app (`jwt` gem, SQLite storage,
  in `oauth-provider/`) issues dev/test RS256 JWTs and exposes a JWKS endpoint
  in the `oauth-provider` Docker Compose service. Cutting over to
  the real IdP later is a config change (JWKS URL/issuer) on the `web` service,
  not a code change. During this stopgap, registering a test client requires two
  steps: register it in `oauth-provider` to get credentials, then register the
  same client_id in the GED admin UI to grant department access.
- **Document upload has two modes** on the same `POST /api/v1/documents`
  endpoint: synchronous (default, returns the document UUID inline) and
  asynchronous (triggered by a `callback_url` param — returns `202 Accepted`
  immediately, stores the file via an ActiveJob background job, then POSTs the
  result to the callback URL). Async exists for large/slow uploads where the
  caller doesn't want to hold the connection open. Kafka-topic delivery was
  considered but deferred — callback URL only for now.
- **No full-text search, no document versioning, no automated backups** in this
  pass — see the spec's Non-goals section before adding these.

## Conventions

- Tests: Minitest (Rails default), WebMock for HTTP stubs. Run `bin/rails test`
  (main app) and `cd oauth-provider && bundle exec rake` (provider).
- Lint: RuboCop rails-omakase — `bin/rubocop` must pass before committing.
- Ruby via mise (`.mise.toml`); dev Postgres via Homebrew (`postgresql@17`).
- Env vars: `GED_OIDC_ISSUER`, `GED_OIDC_JWKS_URL`, `GED_CLIENT_ID_CLAIM`
  (default `sub`), `GED_ADMIN_USERNAME`, `GED_ADMIN_PASSWORD`.
- API error contract: 401 invalid/missing token, 403 unregistered client or
  forbidden department, 404 not found, 400 missing param, 422 validation.
