# GED — Document Management System

Self-hosted document store for a 10+ person team/company, organized around
departments (folders, the permission boundary) and per-document key→value
tags. Accessed mostly by other internal applications over an API secured with
OAuth2/JWT bearer tokens, plus a minimal admin web UI for humans.

Full design: `docs/superpowers/specs/2026-07-08-ged-design.md`. Read it before
making architectural changes — this file is a quick-reference summary, the spec
is the source of truth.

## Stack

- **Ruby on Rails** — single app, API-only controllers under `/api/v1/...` for
  application clients, regular controllers + ERB views for the admin UI
- **PostgreSQL** — app data (departments, documents, app_clients, admins)
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
- **Tags are a key→value map, not a registry:** `documents.tags` is a
  single-level jsonb map (`{"type": "facture", "urgent": null}`, GIN-indexed),
  validated/normalized by the `KvTaggable` concern (also on
  `upload_requests.tags`). Tags are passed at upload (`tags[type]=facture`),
  replaced wholesale via `PUT /api/v1/documents/:id/tags`, and filtered with
  `?tag=<key>` / `?tag=<key>&tag_value=<value>`. There are no `/api/v1/tags`
  endpoints and no admin tags section.

## How the pieces fit (implementation map)

- **Bearer auth chain:** `Api::V1::BaseController#authenticate_app_client!` →
  `Auth::TokenValidator.call` → `Auth::JwksCache` (process-local singleton,
  300s TTL, serves stale keys on refresh failure, fails closed if never
  fetched). Department authorization = `authorize_department!` +
  `AppClient#can_access?` (walks `Department#self_and_ancestors`).
- **Async upload flow:** controller creates an `UploadRequest` (uuid pk) +
  writes raw bytes to `StagingStore` (`storage/staging/<id>`) → 202 →
  `StoreDocumentJob` (runs on the dedicated `uploads` queue) builds the
  `Document`, applies `upload_request.tags`, and deletes the staging file →
  `DeliverCallbackJob` POSTs
  `{ request_id, status, document_id, error_message, tags }` (tags `null` on
  failure) to the `callback_url` (5 attempts, polynomial backoff, logs on
  final failure). `GET /api/v1/upload_requests/:id` mirrors the same payload
  shape for polling.
- **Upload queue pause:** `UploadQueue` (facade over Solid Queue's native
  pause) backs the Pause/Resume button on `/admin/upload_requests`. Pausing
  stops claiming new `StoreDocumentJob`s; running jobs finish and their
  callbacks (on `default`) still deliver. `ReconcileUploadRequestsJob` skips
  pending re-enqueues while paused. Solid Queue runs in every environment
  (dev: inside Puma via `bin/dev`, queue database `ged_development_queue`).
- **Jobs in production run inside Puma** (Solid Queue via
  `SOLID_QUEUE_IN_PUMA=true` in docker-compose); dev uses Solid Queue in Puma
  too; the test env keeps the `:test` adapter but has the queue schema loaded
  (`ged_test_queue`). There is no separate worker container.
- **Document creation is shared, controllers are not:** the admin UI does not
  reuse the API controllers (despite a spec sentence suggesting it) — instead
  `Api::V1::DocumentsController`, `Admin::DocumentsController`, and
  `StoreDocumentJob` all create documents through the `DocumentCreator`
  service. Listing/download logic is still per-controller.

## Known gaps / pitfalls

Correction plan with per-item status:
`docs/superpowers/plans/2026-07-10-ged-corrections.md` (P1 bugs and P2 gaps
fixed 2026-07-10; P3 improvements remain — notably no pagination and
whole-blob-in-memory downloads). Standing pitfalls:

- **Real-IdP cutover checklist:** point `GED_OIDC_ISSUER`/`GED_OIDC_JWKS_URL`
  at the real IdP, set `GED_CLIENT_ID_CLAIM` to the claim carrying the client
  id, and **set `GED_OIDC_AUDIENCE`** — a shared company IdP issues tokens for
  every internal API under the same `iss`, so without `aud` verification any
  app's token would be accepted by GED.
- `active_storage_attachments.record_id` was converted to uuid by a
  corrective migration — Document/UploadRequest use uuid pks, everything else
  bigint. Keep this in mind for any new attachable model.

## Conventions

- Tests: Minitest (Rails default), WebMock for HTTP stubs. Run `bin/rails test`
  (main app) and `cd oauth-provider && bundle exec rake` (provider).
  System tests (Capybara + headless Chrome) are separate:
  `bin/rails test:system`; `bin/ci` runs everything.
- Test quirks: `JwtTestHelpers` (issue_token/stub_jwks!/bearer_header) backs
  all API tests; `Auth::JwksCache.reset!` runs in every setup. WebMock's
  `assert_requested` rejects the `(stub) { block }` form — use
  `assert_requested(:post, url) { block }`.
- Lint: RuboCop rails-omakase — `bin/rubocop` must pass before committing.
- Ruby via mise (`.mise.toml`); dev Postgres via Homebrew (`postgresql@17`).
- Env vars: `GED_OIDC_ISSUER`, `GED_OIDC_JWKS_URL`, `GED_CLIENT_ID_CLAIM`
  (default `sub`), `GED_OIDC_AUDIENCE` (optional; when set, `aud` is
  verified — required at real-IdP cutover), `GED_ADMIN_USERNAME`,
  `GED_ADMIN_PASSWORD`.
- API error contract: 401 invalid/missing token, 403 unregistered client or
  forbidden department, 404 not found, 400 missing param, 422 validation.
- API list endpoints paginate via `limit`/`offset` (default 100, max 500).
