# GED — Document Management System

Self-hosted document store for a 10+ person team/company, organized around
departments (folders, the permission boundary) and per-document key→value
tags. Accessed mostly by other internal applications over an API secured with
OAuth2/JWT bearer tokens, plus a minimal admin web UI for humans.

The application is the Rust crate `ged` (`rust/`). The Rails application was
removed in T16; the local tag `rails-final` (not pushed) is that tree, for
replaying a Rails stack later. Full design of the product:
`docs/superpowers/specs/2026-07-08-ged-design.md`. Migration design:
`docs/superpowers/specs/2026-11-01-rust-migration-design.md`. This file is the
quick reference; those specs stay the historical source for decisions already
taken.

## Workflow (mandatory)

- **Always work in a dedicated git worktree and branch** — never edit directly
  on `main` or in the primary working tree. Each worktree runs its own
  Dockerised Postgres: `bin/worktree-stack db` once per session, then
  `cargo test -p ged` from `rust/`. Concurrent worktrees do not share a
  database. The full integrated stack is `bin/worktree-stack up`.
- **Keep history linear — rebase over merge whenever possible.** Update a
  feature branch from `main` with `git rebase main`. Land finished work with
  a rebase and a fast-forward. Only fall back to a merge commit if a rebase
  would rewrite commits already pushed, and flag that tradeoff instead of
  merging silently.
- **Always get the user's explicit confirmation before the final merge** — do
  not merge back into `main` (or open or complete a PR) without the user
  validating the result.
- **Commit, push, and open PRs only on request.**
- **Finish a feature with a testable, isolated stack.** Rebase the worktree
  branch onto latest `main`, then `bin/worktree-stack up`. Each worktree gets
  its own `COMPOSE_PROJECT_NAME`, host ports and volumes. Let the user
  validate before any merge. On repatriating into `main`, rebase and
  fast-forward, then `bin/worktree-stack clean` and remove the worktree.
- **Green checks before proposing a merge.** `bin/ci` must pass. It runs
  `cargo fmt`, `cargo clippy -D warnings`, `cargo test -p ged`, the
  `oauth-provider` rake suite, `ruby -c` on `bin/perf-upload` and
  `bin/monkey`, compose config, and `docker build` when Docker answers.
  Playwright (`rust/e2e`) and `cargo test -p parity` run only when this
  worktree's stack answers `GET /up`. When nothing answers, `bin/ci` prints
  `Parity: skipped` and `Playwright: skipped` and does not record that skip
  as a pass. The only backend is Rust. `GED_BACKEND=rails` is rejected.
- **Keep docs in sync when scope changes.** Update `docs/api/openapi.yaml`
  when adding or renaming an `/api/v1` route (`rust/ged/tests/guards_api_docs.rs`
  fails otherwise), `docs/architecture/c4.md` when adding a compose service,
  a job, a service module or a queue pool (`rust/ged/tests/guards_c4.rs`),
  and this file when conventions change.
- **Go through specs and plans for architectural changes** — capture the
  design in `docs/superpowers/specs/` and the steps in
  `docs/superpowers/plans/` before touching code.

## Stack

- **Rust** — one binary, `ged`, workspace `rust/`. `ged serve` is the HTTP
  process (Axum). `ged worker` claims jobs. `ged migrate`, `ged seed` and
  `ged run-job` are the operational commands. Pinned in the image at Rust
  1.99.1; local development uses mise (`rust` stable in `.mise.toml`).
- **PostgreSQL 17** — business tables and job tables in one database.
- **Local disk** — blobs and staging files under `/rails/storage` in the
  container (the path name is kept). The compose volume is `storage`.
- **Admin session** — username and password, argon2id, signed cookie. Not
  Devise. `ged seed` writes the admin from `GED_ADMIN_USERNAME` /
  `GED_ADMIN_PASSWORD`. A leftover bcrypt hash is refused until seed rewrites it.
- **JWT** — `jsonwebtoken`, RS256, JWKS fetched by `JwksCache`.
- **OpenTelemetry** — traces and custom metrics over OTLP, disabled unless
  `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Optional `grafana/otel-lgtm` under
  the `otel` compose profile.
- **Docker Compose** — `web` (`ged serve`), `worker` (`ged worker`, scalable),
  `postgres`, `oauth-provider`, and `otel-lgtm` (profile `otel`). One image
  (`Dockerfile`) for web and worker. `bin/worktree-stack up` boots an isolated
  copy. `bin/worktree-stack db` starts only Postgres for host-native tests.
- **Ruby that remains** — `oauth-provider/` (Sinatra, its own Gemfile),
  `bin/perf-upload` and `bin/monkey` (stdlib only). There is no app Gemfile.

## Repo layout

- `rust/ged` — the application
- `rust/parity` — black-box HTTP and business-table suite, Rust only
- `rust/e2e` — Playwright against the admin UI
- `docs/superpowers/specs/` — design specs (historical; do not rewrite them
  to pretend the decisions were always Rust)
- `docs/superpowers/plans/` — implementation plans, including the migration
  plan. Leave them as the record of the work.
- `docs/api/` — `openapi.yaml` and the Bruno collection
- `docs/architecture/c4.md` — C4 diagrams, checked by `guards_c4.rs`
- `docs/migration/` — parity inventory and the accepted gaps. `diff_fuzz`
  replay instructions live in `parity-gaps.md`.
- `oauth-provider/` — temporary OAuth2 dev provider
- `observability/grafana/` — provisioned "GED overview" dashboard
- `bin/worktree-stack`, `bin/ci`, `bin/perf-upload`, `bin/monkey`

## Key architectural decisions

- **Permission model:** departments are folders and the permission boundary.
  Access to a department covers its whole subtree. Each grant carries
  independent `can_read` / `can_write` (at least one true; no access means no
  row). READ gates list, show, download and poll. WRITE gates upload, delete,
  replace-tags and create-sub-department. Write does not imply read: a
  write-only client still receives the document it just created. Read and
  write cascade independently down the subtree; child grants never escalate
  upward. An unknown mode is an error, never a silent read.
- **Two auth paths:** application clients use OAuth2 client_credentials JWTs
  validated as a resource server via JWKS. Admins use the built-in
  username/password session, unrelated to the OIDC flow.
- **Client provisioning is manual.** App clients are registered by an admin
  and mapped to departments. They are never auto-provisioned from JWT claims.
- **The local OAuth2 provider is temporary.** `oauth-provider/` issues dev
  RS256 JWTs and exposes JWKS. Cutting over to the company IdP is a config
  change (`GED_OIDC_*`) on `web` and `worker`, not a code change. Registering
  a test client is two steps: register it in `oauth-provider`, then register
  the same `client_id` in the admin UI.
- **Document upload has two modes** on `POST /api/v1/documents`: synchronous
  (default, returns the document id) and asynchronous (`callback_url` →
  `202`, file staged, `StoreDocumentJob`, then `DeliverCallbackJob` POSTs the
  result). Kafka was considered and deferred.
- **No full-text search, no document versioning, no automated backups.**
- **Tags are a key→value map, not a registry.** `documents.tags` is a
  single-level jsonb object, GIN-indexed. Tags are set at upload, replaced
  wholesale via `PUT /api/v1/documents/:id/tags`, and filtered with
  `?tag=` / `?tag_value=`. There is no `/api/v1/tags` collection.
- **Readonly metadata** is separate from tags: `uploaded_at`, `updated_at`,
  `upload_mode`. `uploaded_at` is sync creation time, or the originating
  request's `created_at` for async. `stored_async` drives `upload_mode`.
  There is no `upload_request_id` on the document. Filters: `uploaded_by`,
  `upload_mode=sync|async`, and ISO8601 `uploaded_after/before`,
  `indexed_after/before`, `updated_after/before` (invalid value → 400).
- **Deleting a document is logical, with a 30-day purge window.** API and
  admin delete set `deleted_at` and `purge_at`. An admin can restore from
  Trash. `PurgeDeletedDocumentsJob` hard-deletes past-due rows and their
  files. `ReconcileUploadRequestsJob` still purges terminal upload requests
  after 30 days.
- **Bulk actions:** the admin documents index has a multi-select toolbar
  (no Stimulus, no Turbo) for bulk soft-delete and a ZIP download streamed
  entry by entry (`DocumentZipper` / `admin::zip_stream`). ZIP entries are
  named `{id}-{filename}`. The API bulk soft-delete is
  `DELETE /api/v1/documents/bulk_destroy` (`document_ids[]`, max 500),
  fail-closed: any id outside the caller's write-accessible departments
  rejects the whole request. There is no bulk-download API.
- **Departments are soft-deleted, purge-gated on references, not a timer.**
  Trashing a department is leaf-only (checked unscoped — a trashed child
  still blocks its parent) and, in one transaction, hides the department and
  bulk-trashes its live documents. A trashed department is API-invisible.
  Restore brings the department and its trashed documents back. The admin
  delete flow offers Move or Trash when the department still holds documents
  or upload requests. Departments with sub-departments stay blocked. There
  is no `departments.purge_at`. `PurgeDeletedDocumentsJob` hard-deletes a
  trashed leaf department only once nothing references it, which cascades
  `client_department_accesses`. Name uniqueness includes trashed rows, so
  recreating a trashed name is a validation error.

## How the pieces fit (implementation map)

Rust modules under `rust/ged/src/`:

- `main.rs`, `lib.rs` — CLI: `serve`, `worker`, `migrate`, `seed`, `run-job`.
- `config.rs` — environment, fail-closed boot in production.
- `db.rs` — SQLx pool. `seed.rs` — admin password (argon2id).
- `telemetry.rs` — `ged.documents.uploaded`, `ged.callbacks.deliveries`,
  `ged.job.duration`. Attribute values stay a closed set.
- `http/` — `error`, `json`, `params` (Rack-shaped query and body),
  `rails_headers` (the response-header behaviour the API still matches),
  `state`.
- `api/` — `documents`, `tags`, `departments`, `upload_requests`, `docs`,
  `ping`, `multipart`, `support`. Routes are `.route("/api/v1…")` literals;
  `guards_api_docs.rs` requires each one in `docs/api/openapi.yaml`.
- `admin/` — `session`, `csrf`, `departments`, `app_clients`, `documents`,
  `upload_requests`, `trash`, `status`, `system_health`, `disk_space`,
  `zip_stream`, `assets`, `view`. Askama templates live next to the screens.
- `auth/` — `token_validator` (`TokenValidator`), `jwks_cache` (`JwksCache`,
  300s TTL, stale on refresh failure, fail closed if never fetched),
  `authenticated_client`.
- `domain/` — `app_client`, `department`, `document`, `document_creator`
  (`DocumentCreator`), `tags`, `upload_request`, `time`.
- `policy/callback_url.rs` — `CallbackUrlPolicy`. Syntactic at request time
  (422, no DNS), and one DNS resolution at delivery. The job connects to the
  approved address and keeps the hostname for Host, SNI and the certificate.
  Numeric IPv4 forms (decimal, hex, octal, short) are normalised in process.
- `storage/` — `disk`, `staging` (`StagingStore`), `files`, `content_type`,
  `disposition`, Marcel tables.
- `jobs/` — `queue` (enqueue, claim with `FOR UPDATE SKIP LOCKED`, pause),
  `worker` (`WorkerSettings`, pools), `pool`, `scheduler`, `store_document`
  (`StoreDocumentJob`), `deliver_callback` (`DeliverCallbackJob`),
  `reconcile` (`ReconcileUploadRequestsJob`), `purge`
  (`PurgeDeletedDocumentsJob`), `heartbeat`, `env`.

**Async upload.** The handler writes an `upload_requests` row and the bytes
to `StagingStore`, returns 202, and enqueues `StoreDocumentJob` on `uploads`.
The job inserts the document and marks the request completed in one
transaction, then deletes the staging file and enqueues `DeliverCallbackJob`
on `default`. The callback body is
`{ request_id, status, document_id, error_message, tags, metadata }`
(tags and metadata null until the document exists). Five attempts, polynomial
backoff. `GET /api/v1/upload_requests/:id` returns the same shape.

**Queues.** Two pools, no `"*"` pool. `uploads` is `JOB_THREADS` threads by
`JOB_CONCURRENCY` processes. `default` is 2 threads, fixed. Pausing uploads
stops new `StoreDocumentJob` claims; a running job finishes, and callbacks
on `default` still deliver. Reconcile does not re-enqueue pending requests
while uploads are paused. `rust/ged/tests/guards_queue.rs` fails if a
`QueueName` is not drained.

**Schedule** (`scheduler::default_schedule`, all on `default`):

- `ReconcileUploadRequestsJob` every 10 minutes
- `clear_finished_jobs` hourly at minute 12
- `PurgeDeletedDocumentsJob` daily at 03:00 UTC

**Scale.** `bin/worktree-stack scale N` is
`docker compose up -d --no-build --no-recreate --scale worker=N worker`.
It does not recreate existing containers. New replicas take the current
shell environment, so export the same `GED_*` variables as for `up`.
Same-host only, because of the shared storage volume.

**API docs** are public: `GET /api/v1/openapi.yaml` and `GET /api/docs`
(Redoc). The YAML is the file in `docs/api/openapi.yaml`, embedded at build.

**Observability.** `/admin/status` is local: database, worker heartbeats,
JWKS health (never fetches on page load), disk via `df`. OpenTelemetry loads
only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Local backend:
`GED_OTEL=1 bin/worktree-stack up` (Grafana on `GED_GRAFANA_PORT`, base 3300;
OTLP on `GED_OTLP_PORT`, base 4318).

## Known gaps / pitfalls

Accepted behavioural gaps, with the measurement that justified each one, are
`docs/migration/parity-gaps.md`. Do not weaken a parity assertion to hide one.

- **Real-IdP cutover.** Point `GED_OIDC_ISSUER` and `GED_OIDC_JWKS_URL` at
  the real IdP, set `GED_CLIENT_ID_CLAIM` (default `sub`), and isolate tokens:
  - Separate audience per service: `GED_OIDC_TOKEN_BINDING=audience` (the
    default) and `GED_OIDC_AUDIENCE`. Production refuses to boot without it.
  - Shared audience: `GED_OIDC_TOKEN_BINDING=scope` and
    `GED_OIDC_REQUIRED_SCOPE`. Every token's `scope` or `scp` must contain it.
    Production refuses to boot without it.
- **Names kept from Rails** so compose and ops did not change: `RAILS_ENV`,
  `SECRET_KEY_BASE`, `SOLID_QUEUE_SHUTDOWN_TIMEOUT`, the storage path
  `/rails/storage`. They are not a Rails runtime.
- **Replaying Rails.** Check out tag `rails-final` in another worktree. That
  tree still has `GED_STACK=rails`. `diff_fuzz` compares a Rails URL and a
  Rust URL; it does not need the Rails sources in this tree. The final
  campaigns (seeds 13 and 29, 100000 requests, `undocumented=0`) are in
  `parity-gaps.md`.
- **Pagination.** `limit`/`offset`, default 100, max 500. `offset` is clamped
  to 1_000_000. `GET /api/v1/documents` returns
  `{ documents, pagination: { limit, offset, total } }`. `total` counts the
  filtered set before limit and offset. `GET /api/v1/departments` is still a
  bare array.
- **API errors.** 401 invalid or missing token, 403 unregistered client or
  forbidden department, 404 not found, 400 missing parameter or malformed
  body, 422 validation, 413 empty body when the upload exceeds
  `GED_MAX_UPLOAD_BYTES`.

## Conventions

- Tests: `cd rust && cargo test -p ged` (needs `bin/worktree-stack db`).
  `cargo test -p parity` needs the compose stack (`GET /up`). Playwright:
  `cd rust/e2e && npx playwright test` with `GED_BASE_URL` and
  `GED_ADMIN_PASSWORD`. Close headless browsers in `finally`; do not leave
  `chromium --headless` behind.
- Lint: `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`
  from `rust/`. No `unsafe` in `ged`.
- `oauth-provider`: `cd oauth-provider && bundle exec rake`.
- `bin/perf-upload` and `bin/monkey`: `ruby -c`. The load driver is
  `bin/perf-upload` against the worktree stack (`--setup` once). Reports in
  `docs/perf/`.
- Ruby and Rust via mise (`.mise.toml`). Dev Postgres is Docker, not Homebrew.
  Stop a leftover `brew services postgresql@17` so it does not hold 5432.
- Env vars: `RAILS_ENV`, `DATABASE_URL`, `SECRET_KEY_BASE`, `GED_OIDC_ISSUER`,
  `GED_OIDC_JWKS_URL`, `GED_CLIENT_ID_CLAIM` (default `sub`),
  `GED_OIDC_TOKEN_BINDING` (`audience` default, or `scope`),
  `GED_OIDC_AUDIENCE` (required in production when binding is audience),
  `GED_OIDC_REQUIRED_SCOPE` (required in production when binding is scope),
  `GED_ADMIN_USERNAME` (default `admin`), `GED_ADMIN_PASSWORD`,
  `GED_MAX_UPLOAD_BYTES` (default 52428800), `OTEL_EXPORTER_OTLP_ENDPOINT`
  (empty disables OTel), `OTEL_SERVICE_NAME` (default `ged`), `JOB_THREADS`
  (default 3), `JOB_CONCURRENCY` (default 1), `SOLID_QUEUE_SHUTDOWN_TIMEOUT`
  (default 30s, must stay under the worker `stop_grace_period` of 40s),
  `PORT` (80 in the container). `GED_BACKEND` is unset or `rust` for parity;
  any other value panics and names the tag `rails-final`.
