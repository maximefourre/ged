# Worker Scaling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract Solid Queue from Puma into a dedicated `worker` Compose
service (same image, `./bin/jobs`), hot-scalable on the same host via
`docker compose up -d --scale worker=N`, with split `uploads`/`default`
worker pools, self-sizing DB pools, graceful shutdown, and a chaos/perf
re-validation quantifying the gain.

**Design spec:** `docs/superpowers/specs/2026-07-13-worker-scaling-design.md`
(read it first — it fixes the pool split rationale, the connection budget,
the compose topology, and the deferred multi-host decision).

## Global Constraints

- Dev (`bin/dev`) keeps Solid Queue in Puma; test keeps the `:test` adapter.
- `bin/ci` must pass; commits only with the user's explicit go-ahead
  (AGENTS.md).
- No API change → `docs/api/openapi.yaml` untouched.

---

### Task 1: Queue + pool + shutdown config

**Files:** `config/queue.yml`, `config/database.yml`,
`config/environments/production.rb`

- [x] `queue.yml`: replace the single `"*"` pool with `uploads`
      (`JOB_THREADS`/`JOB_CONCURRENCY` env knobs, polling 0.1) + fixed
      `default` (2 threads, 1 process, polling 0.5); comment why `"*"` is
      banned (global FIFO starvation) and point at the guard test.
- [x] `database.yml`: `max_connections` =
      `max(RAILS_MAX_THREADS, JOB_THREADS + 2)`.
- [x] `production.rb`: `config.solid_queue.shutdown_timeout` from
      `SOLID_QUEUE_SHUTDOWN_TIMEOUT` (default 30s).

### Task 2: Compose topology

**Files:** `docker-compose.yml`

- [x] New `worker` service: `./bin/jobs`, `storage` volume, env
      (`DATABASE_URL`, `SECRET_KEY_BASE`, `RAILS_ENV`, `JOB_THREADS`,
      `JOB_CONCURRENCY`, `GED_OIDC_ISSUER`, `GED_OIDC_JWKS_URL`), no
      ports/`container_name`, `stop_grace_period: 40s`, `depends_on`
      postgres healthy + web healthy (web's entrypoint runs `db:prepare`).
- [x] `web`: remove `SOLID_QUEUE_IN_PUMA`; add `/up` curl healthcheck
      (`start_period` covers first-boot `db:prepare`).
- [x] `postgres`: `max_connections=200`.

### Task 3: Queue coverage guard test

**Files:** `test/jobs/queue_coverage_test.rb`

- [x] Parse `config/queue.yml` (ERB → YAML), collect every worker pool's
      queues; eager-load `app/jobs`; assert each `ApplicationJob`
      descendant's `queue_name` is covered.
- [x] Verify RED: an uncovered fake job must fail the test.

### Task 4: Tooling

**Files:** `bin/worktree-stack`, `bin/perf-upload`

- [x] `worktree-stack` usage comment: `up [--scale worker=N]`.
- [x] `perf-upload`: `--chaos-target web|worker` (default `web`);
      kill/start the chosen service; skip web `wait_for_health` when the
      target is `worker`.

### Task 5: Docs

**Files:** `AGENTS.md`

- [x] Replace the "Jobs in production run inside Puma" bullet with the
      dedicated `worker` service + scaling runbook (dev still in-Puma via
      `bin/dev`).
- [x] Update the Stack/Compose services list and the Env vars list
      (`JOB_THREADS`, `JOB_CONCURRENCY`, `SOLID_QUEUE_SHUTDOWN_TIMEOUT`).

### Task 6: Validation

- [x] `bin/ci` green (report actual output).
- [x] Stack up (1 worker): `perf-upload --setup`, `--profile mix --mode
      async`, `--profile many-small` — expect baselines ≈ 2,901 / 6,168
      stored/min (no regression from moving out of Puma).
      → measured 3,105 / 6,674 stored/min: no regression.
- [x] Hot scale to 3 workers (via the new `bin/worktree-stack scale N`,
      added because `up --build --scale` recreates containers when the
      build context changed): rerun both + `few-large`.
      → mix 5,580 (+80 %), many-small 10,735 (+61 %), few-large 630
      (+139 % — NOT disk-bound as previously assumed; the ceiling was the
      single worker process at ~264 MB/s). Zero-loss OK everywhere.
- [x] Zero-worker degradation: 20 async POSTs with workers stopped → all
      202 + pending; drained to completed <10 s after `start worker`.
      Pause/Resume exercised via the `UploadQueue` facade with 3 replicas:
      10 pending while paused, all completed ~6 s after resume.
- [x] Recurring dedupe under 3 replicas: 2 ticks fired, each exactly once;
      duplicate query returns 0.
- [x] Chaos: `--chaos` (web) and `--chaos --chaos-target worker`.
      → web: 546/546 accepted requests completed, zero loss, web back 3 s
      after SIGKILL, workers kept draining meanwhile. worker: all 1,000
      requests completed, zero documents lost, 2 at-least-once duplicates
      (documented semantics; the tool's strict check prints FAILED on
      duplicates), 6 stragglers recovered by the 10-min reconcile guard.
- [x] Write `docs/perf/2026-07-13-worker-scaling.md` (1 vs 3 workers,
      existing report format).

### Task 7: Close the at-least-once duplicate window (follow-up, same branch)

**Files:** `app/jobs/store_document_job.rb`,
`test/jobs/store_document_job_test.rb`

- [x] TDD RED: deterministic test crashing the job between the document
      INSERT and the completed UPDATE — asserts no document/blob survives.
      → failed as expected on unpatched code (1 document left behind).
- [x] GREEN: wrap both writes in one `ApplicationRecord.transaction`,
      nested *inside* `File.open` (Active Storage defers the blob's disk
      write to commit time — commit outside the block raises
      `IOError: closed stream`).
- [x] Blob content verified end-to-end after the deferred-upload change
      (download == staged bytes); `bin/ci` green.
- [x] Post-fix `--chaos --chaos-target worker` rerun on the rebuilt stack.
      → 1,000/1,000 completed, exactly 1,000 documents (distinct filenames
      verified in SQL): zero loss, zero duplicates, `zero-loss: OK`.
