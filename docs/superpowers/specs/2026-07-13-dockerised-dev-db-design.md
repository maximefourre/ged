# Dockerised dev/test Postgres (drop Homebrew) — Design

**Date:** 2026-07-13
**Status:** Approved (design validated with the user)

## Context

`bin/rails test` and `bin/dev` ran against a **shared Homebrew Postgres**:
`config/database.yml` dev/test carried no host/port/user/password, so the `pg`
adapter connected over the local Unix socket as the OS user — i.e. the
`postgresql@17` brew service. The dependency was implicit (it lived in the
*absence* of connection settings) but real, and documented in the README /
AGENTS.md.

The problem it caused: **two git worktrees running concurrently collided in that
single Postgres**. They shared one `schema_migrations`, so a migration at the
same version number was silently skipped in the second worktree, and
`db/schema.rb` dumps picked up the other worktree's columns. This bit the
document-soft-delete work (collided with document-readonly-metadata's
`20260712000001`). The stopgap — a hardcoded `ged_sd_*` edit to
`config/database.yml` — was uncommittable.

`bin/worktree-stack` already isolates the **Docker** stack per worktree
(`COMPOSE_PROJECT_NAME` + slug-derived host ports + namespaced volumes), but the
dev/test database sat outside that isolation.

## Decision

Run Postgres in Docker **all the time** and drop the Homebrew dependency. Two
choices, made with the user:

1. **Ruby stays native (mise).** `bin/rails test`/`bin/dev`/`console` keep
   running on the host — fast, with working system tests (headless Chrome) and
   debugger. Only the **database** moves into Docker.
2. **One Postgres container per worktree.** Each worktree's stack publishes its
   own Postgres on a slug-derived host port (same offset already used for
   web/oauth), and `config/database.yml` **auto-derives** that port. Isolation =
   separate containers + volumes; database names stay `ged_development` /
   `ged_test` (+ `_queue`).

This kills the collision by reusing the existing `worktree-stack` isolation,
without slowing the native test loop.

## Goals

- No Homebrew Postgres anywhere in the dev/test path; the `postgres` Compose
  service is the single source for local databases.
- `bin/rails test` / `bin/dev` "just work" from any worktree, each hitting its
  own isolated Postgres — no cross-worktree migration/schema clashes.
- Production (`DATABASE_URL`) and CI (service container) paths unchanged / green.

## Non-goals

- Not running Rails itself in Docker for dev/test (rejected: slow macOS I/O,
  harder system tests + debugger). The full `bin/worktree-stack up` stack (web
  in production mode) remains only for pre-merge integrated validation.
- No change to the `web`/`oauth-provider` services or the production multi-DB
  (`ged_production` / `_cache` / `_queue`) setup.

## Design

- **`docker-compose.yml`**: publish `"${GED_DB_PORT:-5432}:5432"` on `postgres`.
- **`bin/worktree-stack`**: derive `db_port` (primary → 5432, worktree →
  `5432 + offset`), `export GED_DB_PORT`; add `db` (start only Postgres, wait
  for `pg_isready`) and `db-port` (print the port) subcommands. `db` calls
  `ensure_env` because Compose interpolates the whole file even for a
  single-service start (`web`'s `${SECRET_KEY_BASE:?}`).
- **`config/database.yml`**: a `local_pg` anchor (kept off `default` so
  production stays `DATABASE_URL`-only) sets host/port/user/password for dev+test
  primary+queue. The port is `ENV.fetch("GED_DB_PORT") { … bin/worktree-stack
  db-port … }` with a `rescue`/`nonzero?` fallback to 5432. Single source of
  truth for the offset math = `bin/worktree-stack`.
- **`bin/setup`**: start the Docker Postgres (`bin/worktree-stack db`) before
  `db:prepare`. `bin/ci` inherits this via `bin/setup --skip-server`.
- **`.github/workflows/ci.yml`**: set `GED_DB_PORT: 5432` in the test jobs to
  short-circuit the lookup (CI's service container is always on 5432).

## Key facts (verified)

- `pg` 1.6.3 precompiled `arm64-darwin` **bundles libpq** → dropping
  `postgresql@17` doesn't break the gem; no `brew install libpq` needed.
- Rails 8.1 sets `erb.filename` when rendering `database.yml`, so `__dir__`
  resolves to `config/` → `File.expand_path("../bin/worktree-stack", __dir__)`
  is reliable (no CWD assumption).
- `DATABASE_URL` fallback applies only to the `primary` connection; the `queue`
  connection reads the YAML — so the new explicit `localhost:5432/postgres`
  defaults make CI's queue connection *more* explicit, not broken. Zero
  regression.
- **Port 5432 clash**: a still-running `brew services postgresql@17` holds
  `127.0.0.1:5432`, so the primary-tree `docker compose up -d postgres` fails to
  bind (or worse, Rails connects to brew). Stopping brew PG is a documented
  prerequisite. Worktrees (5433+) don't clash.

## Verification

Proven end-to-end in the `dockerize-dev-db` worktree (port 5481): `db:prepare`
created all four dev/test DBs on the Docker Postgres, `bin/rails test` (199) and
`bin/ci` (Rails + 13 system tests + rubocop + brakeman + seeds) passed against
it; the ERB resolved 5481 with `GED_DB_PORT` unset and short-circuited to 5432
when set.
