# Dockerised dev/test Postgres (drop Homebrew) — Implementation

**Date:** 2026-07-13
**Design:** `docs/superpowers/specs/2026-07-13-dockerised-dev-db-design.md`
**Status:** Implemented on branch `chore/dockerize-dev-db` (shared tooling — lands
standalone, not bundled into a feature branch).

## Steps

- [x] **1. `docker-compose.yml`** — publish `"${GED_DB_PORT:-5432}:5432"` on the
  `postgres` service (comment: host-native dev/test connect here; per-worktree
  via `GED_DB_PORT`).
- [x] **2. `bin/worktree-stack`** — derive `db_port` in both branches
  (primary `${GED_DB_PORT:-5432}`, worktree `${GED_DB_PORT:-$((5432+offset))}`),
  `export GED_DB_PORT`, show `db=` in the `up`/`urls` echo, add subcommands:
  - `db)` → `ensure_env` → `docker compose up -d postgres` → wait on
    `docker compose exec -T postgres pg_isready` → "ready".
  - `db-port)` → `echo "$db_port"` (only the number; consumed by database.yml).
  - update the header usage comment.
- [x] **3. `config/database.yml`** — add the `local_pg` anchor (host/port/user/
  password; port via `ENV.fetch("GED_DB_PORT") { backtick bin/worktree-stack
  db-port … rescue 5432 }`); point dev+test primary+queue at `*local_pg`.
  `default` + production untouched.
- [x] **4. `bin/setup`** — run `bin/worktree-stack db` before `bin/rails
  db:prepare` (idempotent; `bin/ci` inherits via `--skip-server`).
- [x] **5. `.github/workflows/ci.yml`** — add `GED_DB_PORT: 5432` to the `test`
  and `system-test` job `env:` blocks (short-circuits the port lookup).
- [x] **6. Docs** — `README.md` install section (Docker Postgres + "stop brew PG
  first" warning) and `AGENTS.md` (Conventions line 238, Stack, Workflow bullet).
- [x] **7. Spec + plan** — this file + the design doc.
- [ ] **8. Memory** — mark `per-worktree-local-db-isolation` RESOLVED (out of
  repo, in `~/.claude/.../memory/`; done outside the branch).

## Verification (done)

- [x] `bin/worktree-stack db-port` prints the derived port (5481 here); `bash -n`
  clean; worktree detection correct.
- [x] `config/database.yml` ERB → dev/test resolve to `localhost:5481/postgres`
  with `GED_DB_PORT` unset; short-circuit to 5432 when set; production url empty.
- [x] `bin/worktree-stack db` starts Postgres (isolated volume, port 5481),
  waits for healthy.
- [x] `bin/rails db:prepare` creates the four dev/test DBs; Rails confirms
  `connection_db_config.port == 5481`.
- [x] `bin/rails test` → 199 runs, 0 failures.
- [x] `bin/ci` → all steps green (setup via `bin/worktree-stack db`, rubocop,
  gem/importmap audit, brakeman, 199 Rails tests, 13 system tests, seeds).

## Rollout notes

- **Stop any Homebrew Postgres first** (`brew services stop postgresql@17`) — it
  holds port 5432 and would break the primary-tree `docker compose up -d
  postgres`.
- Existing worktrees: run `bin/worktree-stack db` + `bin/rails db:prepare` once
  (fresh Docker volumes; brew data not carried over).
- Landing: shared tooling — rebase onto latest `main`, `bin/ci` green, user
  confirmation, then rebase + fast-forward merge and `bin/worktree-stack clean`.
