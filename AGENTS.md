# GED — Document Management System

Self-hosted document store for a 10+ person team/company, organized around
departments (folders, the permission boundary) and per-document key→value
tags. Accessed mostly by other internal applications over an API secured with
OAuth2/JWT bearer tokens, plus a minimal admin web UI for humans.

Full design: `docs/superpowers/specs/2026-07-08-ged-design.md`. Read it before
making architectural changes — this file is a quick-reference summary, the spec
is the source of truth.

## Workflow (mandatory)

- **Always work in a dedicated git worktree and branch** — never edit directly
  on `main` or in the primary working tree. Create an isolated worktree +
  feature branch for every task before making changes. Each worktree runs its
  own Dockerised dev/test Postgres — `bin/worktree-stack db` once per session,
  then `bin/rails test`/`bin/dev` natively — so concurrent worktrees never
  collide on a shared database (that's separate from the full integrated
  `bin/worktree-stack up` stack used for pre-merge validation).
- **Keep history linear — rebase over merge whenever possible** — update a
  feature branch from `main` with `git rebase main` (never `git merge main`
  into it), and land finished work with a rebase + fast-forward
  (`git rebase main` then `git merge --ff-only`, or an equivalent
  rebase/squash PR merge) instead of a merge commit. Only fall back to a real
  merge commit if a rebase would rewrite commits already pushed/shared in a
  way the user doesn't want rewritten — flag that tradeoff to the user instead
  of silently merging.
- **Always get the user's explicit confirmation before the final merge** — do
  not merge back into `main` (or open/complete a PR) without validating the
  result with the user first.
- **Commit, push, and open PRs only on request** — never commit/push/merge
  proactively; wait for the user's explicit go-ahead (consistent with the
  merge-confirmation rule above).
- **Finish a feature with a testable, isolated stack** — when the work in a
  worktree is done, finalize it on the worktree branch (rebase onto latest
  `main` there, not the other way around) and bring up its own Docker stack
  with `bin/worktree-stack up`. Each worktree gets an isolated
  `COMPOSE_PROJECT_NAME` + host ports (data volumes are namespaced too), so it
  runs alongside `main` and other worktrees for independent testing. Let the
  user validate against that stack before any merge. On repatriating into
  `main`, rebase + fast-forward merge (see above), then do the ménage:
  `bin/worktree-stack clean` (drops its containers, volumes, and
  locally-built images), then remove the worktree.
- **Green checks before proposing a merge** — `bin/ci` must pass (at minimum
  `bin/rails test` + `bin/rubocop`); report the actual output, don't claim
  success without running it.
- **Keep docs in sync when scope changes** — update `docs/api/openapi.yaml`
  when adding/renaming an `/api/v1` endpoint (a test fails otherwise) and this
  `AGENTS.md` when conventions change.
- **Go through specs/plans for architectural changes** — capture the design in
  `docs/superpowers/specs/` and the steps in `docs/superpowers/plans/` before
  touching code, rather than improvising.

## Stack

- **Ruby on Rails** — single app, API-only controllers under `/api/v1/...` for
  application clients, regular controllers + ERB views for the admin UI
- **PostgreSQL** — app data (departments, documents, app_clients, admins)
- **Active Storage** (local disk service) — document file storage
- **Devise** — admin UI username/password session login
- **`jwt` gem** — validates bearer tokens from app clients against a JWKS endpoint
- **OpenTelemetry** — traces + custom metrics over OTLP, entirely disabled
  unless `OTEL_EXPORTER_OTLP_ENDPOINT` is set (see the implementation map);
  optional `grafana/otel-lgtm` backend under the `otel` compose profile
- **Docker Compose** — `web` (Rails/Puma), `worker` (Solid Queue, scalable
  via `--scale worker=N`), `postgres`, `oauth-provider`;
  `bin/worktree-stack up` boots an isolated per-worktree copy for testing (see
  the Workflow section). `postgres` also publishes a host port
  (`${GED_DB_PORT:-5432}`), so the **dev/test database always runs in Docker**
  (no Homebrew Postgres): `bin/worktree-stack db` starts just Postgres for the
  native `bin/rails test`/`bin/dev` loop, and `db-port` prints the port
  `config/database.yml` auto-derives.

## Repo layout

- `docs/superpowers/specs/` — design specs (source of truth for architecture
  decisions)
- `docs/superpowers/plans/` — implementation plans (checkbox progress tracking,
  designed for resuming and/or delegation to subagents)
- `docs/api/` — `openapi.yaml` (OpenAPI 3.1 contract, hand-maintained source of
  truth for the API) and `bruno/` (ready-to-run Bruno collection)
- `oauth-provider/` — temporary standalone OAuth2 dev provider (own Gemfile)

## Key architectural decisions

- **Permission model:** departments are folders and the permission boundary.
  Access to a department covers its whole subtree — no finer-grained
  per-subfolder permissions. Each grant (`client_department_accesses` row)
  carries independent `can_read` / `can_write` booleans (DB check: at least
  one true; "no access" = no row). READ gates list/show/download/poll, WRITE
  gates upload/delete/replace-tags/create-sub-department; write does **not**
  imply read (a write-only client still gets the created document echoed back
  — that's the result of its write, not a read). Read and write cascade
  independently down the subtree; child grants never escalate upward. The
  admin app-client form is a per-department Read/Write checkbox grid.
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
- **Readonly document metadata:** every document also exposes a system-managed,
  readonly `metadata` block (`uploaded_at`, `updated_at`, `upload_mode`),
  serialized separately from the client-owned `tags` map so replace-the-whole-map
  semantics stay intact. `uploaded_at` is a denormalized column (sync = creation
  time; async = the originating request's `created_at`, passed by
  `StoreDocumentJob`); the indexation date is the top-level `created_at` and the
  owner is `uploaded_by` (not duplicated). `stored_async` is a plain boolean flag
  that drives `upload_mode` — deliberately no `upload_request_id` link, so those
  rows can be purged later. `Document#metadata_hash` is the single serialized
  shape, reused by `document_json` and the async callback/polling payloads.
  Documents are filterable by `?uploaded_by=`, `?upload_mode=sync|async`, and
  ISO8601 ranges `?uploaded_after/before`, `?indexed_after/before`,
  `?updated_after/before` (invalid value → 400).
- **Deleting a document is logical, with a 30-day purge window:** `DELETE
  /api/v1/documents/:id` and the admin delete soft-delete (hide the document,
  keep its file) instead of destroying it, so an admin can restore it from the
  Trash within the retention window; a scheduled job hard-deletes documents once
  the window elapses. The parallel async **upload_request** purge already exists
  (`ReconcileUploadRequestsJob`, 30-day terminal-request cleanup) and is
  unchanged.

## How the pieces fit (implementation map)

- **Bearer auth chain:** `Api::V1::BaseController#authenticate_app_client!` →
  `Auth::TokenValidator.call` → `Auth::JwksCache` (process-local singleton,
  300s TTL, serves stale keys on refresh failure, fails closed if never
  fetched). Department authorization = `authorize_department!(dept, mode:)` +
  `AppClient#can_access?(dept, mode)` — `mode` (`:read`/`:write`) is required
  on both, so a forgotten mode fails loudly instead of silently authorizing
  the wrong verb (walks `Department#self_and_ancestors` over the grants where
  the mode's column is true). List scopes use
  `accessible_department_ids(:read)`; `find_authorized_document(mode:)` splits
  show/download (`:read`) from destroy (`:write`).
- **Async upload flow:** controller creates an `UploadRequest` (uuid pk) +
  writes raw bytes to `StagingStore` (`storage/staging/<id>`) → 202 →
  `StoreDocumentJob` (runs on the dedicated `uploads` queue) builds the
  `Document` and marks the request completed **in one transaction** (a crash
  in between would otherwise let reconcile re-store the document —
  crash-recovery is exactly-once for documents, at-least-once for job runs
  and callbacks), applies `upload_request.tags`, and deletes the staging
  file →
  `DeliverCallbackJob` POSTs
  `{ request_id, status, document_id, error_message, tags, metadata }` (tags and
  metadata `null` until the document exists) to the `callback_url` (5 attempts,
  polynomial backoff, logs on final failure). `GET /api/v1/upload_requests/:id`
  mirrors the same payload shape for polling. **SSRF guard:**
  `CallbackUrlPolicy` blocks non-routable `callback_url` targets (loopback,
  RFC1918, link-local incl. cloud metadata, CGNAT, IPv4-mapped IPv6) —
  syntactically at request time in `UploadRequest` (422, no DNS) and with DNS
  resolution in `DeliverCallbackJob` (refuse + log, no retry). Request-time
  parsing also normalizes numeric IPv4 encodings itself via an `inet_aton`
  helper (decimal `2130706433`, hex `0x7f000001`, octal `0177.0.0.1`, short
  `127.1`), so numeric-loopback forms are caught before any resolver — and
  identically on Darwin and glibc, which disagree on octal. At delivery,
  `decide(url, resolve: true)` resolves **once** via `Addrinfo.getaddrinfo`
  (the same libc path `Net::HTTP` uses) and the job **pins** the approved
  address (`http.ipaddr=`, keeping the hostname for Host/SNI/cert), so there is
  no resolve/connect rebinding window; an unresolvable host is refused (nothing
  to pin). Design: `docs/superpowers/specs/2026-07-15-callback-ssrf-pinning-design.md`;
  security audit findings live in `docs/security/`.
- **Upload queue pause:** `UploadQueue` (facade over Solid Queue's native
  pause) backs the Pause/Resume button on `/admin/upload_requests`. Pausing
  stops claiming new `StoreDocumentJob`s; running jobs finish and their
  callbacks (on `default`) still deliver. `ReconcileUploadRequestsJob` skips
  pending re-enqueues while paused. Solid Queue runs in every environment
  (dev: inside Puma via `bin/dev`, queue database `ged_development_queue`).
- **Jobs in production run in a dedicated `worker` compose service**
  (`./bin/jobs`, same image as web), hot-scalable on the same host with
  `bin/worktree-stack scale N` (= `docker compose up -d --no-build
  --no-recreate --scale worker=N worker`) — no rebuild, web and existing
  workers untouched; replicas coordinate via Postgres. (`up --scale
  worker=N` also works but its `--build` recreates containers whenever the
  build context changed.) `config/
  queue.yml` splits two pools: `uploads` (env knobs `JOB_THREADS` ×
  `JOB_CONCURRENCY`) and a small fixed `default` — **no `"*"` pool** (would
  let an uploads backlog starve callbacks/reconcile); every `queue_as` must
  be listed there, enforced by `test/jobs/queue_coverage_test.rb`. Web and
  worker share the `storage` volume (staging + Active Storage blobs), so
  scaling is same-host only — multi-host needs object storage first (see
  `docs/superpowers/specs/2026-07-13-worker-scaling-design.md`). Dev still
  runs Solid Queue inside Puma (`bin/dev` sets `SOLID_QUEUE_IN_PUMA`); the
  test env keeps the `:test` adapter but has the queue schema loaded
  (`ged_test_queue`).
- **Document creation is shared, controllers are not:** the admin UI does not
  reuse the API controllers (despite a spec sentence suggesting it) — instead
  `Api::V1::DocumentsController`, `Admin::DocumentsController`, and
  `StoreDocumentJob` all create documents through the `DocumentCreator`
  service. Listing/download logic is still per-controller.
- **Soft delete + purge:** `Document#soft_delete!` sets `deleted_at` +
  `purge_at` (= `+ Document::RETENTION`, 30 days); a
  `default_scope { where(deleted_at: nil) }` hides soft-deleted rows on every
  read path, with `with_deleted` / `deleted` / `purgeable` (`unscope`) as the
  escape hatches used by the Trash and the purge job. Admin Trash is
  `Admin::DeletedDocumentsController` (`index` over `Document.deleted`,
  `restore` → `Document#restore!`), linked in the top-bar nav.
  `PurgeDeletedDocumentsJob` (`Document.purgeable.find_each(&:destroy!)`,
  scheduled daily in `config/recurring.yml`) hard-deletes past-due rows;
  `destroy!` fires Active Storage's default async blob purge. Deleting a
  department that holds only soft-deleted docs stays safe — the
  `documents.department_id` FK backstops the (default-scoped)
  `restrict_with_error`, caught by `Admin::DepartmentsController`.
- **Admin UI is a hand-rolled design system, no framework:** one plain
  `app/assets/stylesheets/application.css` (CSS custom-property tokens, served
  by Propshaft, no build) styles a top-bar layout via class names on the views.
  Light/dark is token-driven (`:root` vs `:root[data-theme="dark"]`), set before
  paint by an inline script in the layout `<head>` and toggled by
  `#theme-toggle` (persisted in `localStorage`, handled in the importmap
  `application.js` — the only JS, no Stimulus/Turbo). The admin document upload
  is a drag-and-drop dropzone (`[data-dropzone]` + `[data-dropzone-file]`,
  progressive enhancement over a visible `<input type=file>` so Capybara's
  `attach_file` still works). Tags render as key/value chips via
  `admin/shared/_tag_chips`; upload-request status uses
  `AdminHelper#status_badge_class`. Keep visible labels/field names stable —
  `test/system/admin_ui_test.rb` and `test/controllers/admin/*` assert on them.
- **API docs are served, hand-maintained:** `ApiDocsController`
  (`ActionController::Base`, no auth — deliberately public) serves the committed
  `docs/api/openapi.yaml` at `GET /api/v1/openapi.yaml` and a Redoc page at
  `GET /api/docs`. The spec is written by hand; `test/integration/api_docs_test.rb`
  fails if a routed `/api/v1` path is missing from it (structure only, not
  params/shapes) — update the YAML when adding or renaming an endpoint.
- **Observability is two independent layers.** (1) Admin status page
  `/admin/status` (`Admin::StatusController` + `SystemHealth`/`DiskSpace`
  services): system health checks (DB, Solid Queue worker heartbeats, JWKS via
  the read-only `Auth::JwksCache#health` — never fetches, so no HTTP on page
  load; storage disk via `df -Pk`), pipeline and document counts — all local,
  zero dependencies. (2) OpenTelemetry: traces (Rails/PG-obfuscated/Net::HTTP
  auto-instrumentation) + custom metrics behind the `Telemetry` facade
  (`lib/telemetry.rb` — `ged.documents.uploaded`, `ged.callbacks.deliveries`,
  `ged.job.duration`; attribute values must stay closed-set, never
  filenames/tags/departments). `config/initializers/opentelemetry.rb` loads
  the SDK **only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set** — the facade's
  instruments are no-ops otherwise (only `opentelemetry-metrics-api` is loaded
  at boot; the metrics SDK/exporter are experimental 0.x, minor-pinned).
  Tests: `test/lib/telemetry_test.rb` guards the SDK stays dormant in test;
  `TelemetryTestHelpers#capture_telemetry` records emissions. Local backend:
  `grafana/otel-lgtm` compose service under the `otel` profile
  (`GED_OTEL=1 bin/worktree-stack up`; Grafana on `GED_GRAFANA_PORT` base
  3300, OTLP on `GED_OTLP_PORT` base 4318), provisioned "GED overview"
  dashboard in `observability/grafana/` (Prometheus mangles metric names:
  `ged_documents_uploaded_total` etc.; HTTP panels use Tempo span metrics —
  Ruby emits traces only).

## Known gaps / pitfalls

Correction plan with per-item status:
`docs/superpowers/plans/2026-07-10-ged-corrections.md` (P1 bugs, P2 gaps,
and P3 improvements fixed 2026-07-10, except P3-8 — `accessible_department_ids`
perf, deliberately deferred until department counts grow). Standing
pitfalls:

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
- Perf/resilience tooling: `bin/perf-upload` (stdlib-only load driver) runs
  bulk-upload campaigns and a `--chaos` crash-recovery test against the
  worktree Docker stack (`--setup` provisions the test client); campaign
  reports live in `docs/perf/`. Design:
  `docs/superpowers/specs/2026-07-13-perf-upload-design.md`.
- Ruby via mise (`.mise.toml`). Dev/test Postgres runs in Docker — **no
  Homebrew Postgres**. Start it with `bin/worktree-stack db` (one container per
  worktree on a slug-derived host port; `config/database.yml` auto-derives that
  port via `bin/worktree-stack db-port`, so `bin/rails test`/`bin/dev` connect
  natively to the right container). Stop any leftover `brew services
  postgresql@17` first — it would hold port 5432 for the primary tree.
- Env vars: `GED_OIDC_ISSUER`, `GED_OIDC_JWKS_URL`, `GED_CLIENT_ID_CLAIM`
  (default `sub`), `GED_OIDC_AUDIENCE` (optional; when set, `aud` is
  verified — required at real-IdP cutover), `GED_ADMIN_USERNAME`,
  `GED_ADMIN_PASSWORD`, `OTEL_EXPORTER_OTLP_ENDPOINT` (empty = OTel fully
  disabled; compose feeds it from `GED_OTEL_ENDPOINT`), `OTEL_SERVICE_NAME`
  (default `ged`). Worker scaling: `JOB_THREADS` (default 3, uploads
  pool threads; also raises the DB pool via `config/database.yml`),
  `JOB_CONCURRENCY` (default 1, uploads worker processes per container),
  `SOLID_QUEUE_SHUTDOWN_TIMEOUT` (default 30s, must stay under the worker's
  `stop_grace_period` 40s).
- API error contract: 401 invalid/missing token, 403 unregistered client or
  forbidden department, 404 not found, 400 missing param, 422 validation.
- API list endpoints paginate via `limit`/`offset` (default 100, max 500;
  `offset` is clamped to `MAX_OFFSET` = 1,000,000 so a hostile value cannot
  overflow the Postgres bigint offset into a 500).
