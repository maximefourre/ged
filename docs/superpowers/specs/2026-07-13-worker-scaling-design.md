# Solid Queue worker scaling — dedicated `worker` service — Design

**Date:** 2026-07-13
**Status:** approved (planned + validated in session)

## Problem

The 2026-07-13 perf campaigns (`docs/perf/`) measured the async upload
pipeline and found the expected bottleneck: the single Solid Queue worker
running **inside Puma** (`SOLID_QUEUE_IN_PUMA=true`, `JOB_CONCURRENCY=1`,
3 threads). On the `mix` profile, ingest accepts 7,375 docs/min while the
worker stores only 2,901 docs/min — a backlog builds behind one worker
(`many-small`: 19,925 vs 6,168). The reports deliberately left tuning as a
follow-up decision; this is that follow-up.

The operational question: absorb load by spinning up more workers
dynamically **on the same instance**, or scale horizontally across hosts?

## Decision

Extract Solid Queue from Puma into a dedicated `worker` Docker Compose
service (same image, `command: ["./bin/jobs"]`), scaled **on the same host**
with `docker compose up -d --scale worker=N`:

- **Hot scaling, no restarts.** Compose adds/removes worker containers
  without touching `web` or the existing workers. Replicas coordinate
  through Postgres (`FOR UPDATE SKIP LOCKED`) — no extra broker.
- **Manual trigger.** A documented runbook command, no queue-depth
  autoscaler. The async queue already absorbs bursts gracefully (10,000
  pending jobs with flat POST latency, measured); scaling is for draining a
  persistent backlog faster, not for surviving spikes.
- **Web and jobs stop competing.** Puma threads serve requests; workers burn
  CPU/disk in their own containers.

**Multi-host distribution is explicitly deferred.** Both async staging
(`storage/staging/<uuid>`, written by the controller, read+deleted by
`StoreDocumentJob`, swept by `ReconcileUploadRequestsJob`) and Active
Storage blobs (`:local` Disk service, written by workers on async store,
read by web for downloads via `send_file`) live on the single shared
`storage:/rails/storage` named volume. Same-host containers can share that
volume; separate hosts cannot. Going distributed requires an S3-compatible
object store for blobs **plus** a staging redesign — premature at the
current scale, where large-file throughput is disk-bound (~280 MB/s)
anyway. Revisit if one machine stops being enough.

## Design

### Two worker pools instead of `"*"` (`config/queue.yml`)

```yaml
workers:
  - queues: uploads       # StoreDocumentJob — the scalable pool
    threads: <%= ENV.fetch("JOB_THREADS", 3) %>
    processes: <%= ENV.fetch("JOB_CONCURRENCY", 1) %>
    polling_interval: 0.1
  - queues: default       # callbacks, reconcile, purge — small fixed pool
    threads: 2
    processes: 1
    polling_interval: 0.5
```

Why not keep `queues: "*"`: verified in solid_queue 1.4.0, `QueueSelector`
treats any list containing `"*"` as **one** relation ordered globally by
`(priority, job_id)` — a 10k `uploads` backlog would starve `default`
(callbacks, reconcile). List-order priority only works with explicit queue
names. Isolation also cuts the other way: slow `DeliverCallbackJob` HTTP
calls can't occupy upload threads.

Dropping the `"*"` safety net creates an orphaned-queue risk (a new
`queue_as` nobody drains), guarded by `test/jobs/queue_coverage_test.rb`:
every `ApplicationJob` descendant's `queue_name` must appear in some worker
pool. Active Storage purge jobs ride `default` (no
`config.active_storage.queues.purge` override exists) — covered.

### Recurring tasks under N replicas

Every replica runs the full supervisor (dispatcher + scheduler + workers).
Safe: `solid_queue_recurring_executions` has a unique index on
`[task_key, run_at]` (`db/queue_schema.rb:83`) and
`RecurringExecution.create_or_insert!` rescues `RecordNotUnique`, so N
schedulers enqueue each task exactly once. No pinning, no config asymmetry
between replicas.

### Connection-pool budget

`config/database.yml` pool becomes self-sizing:

```erb
max_connections: <%= [ ENV.fetch("RAILS_MAX_THREADS", 5).to_i,
                       ENV.fetch("JOB_THREADS", 3).to_i + 2 ].max %>
```

Solid Queue validates at boot that the queue-DB pool ≥ max worker threads
+ 2 and fails loudly; the formula removes the "raised `JOB_THREADS`, forgot
`RAILS_MAX_THREADS`" footgun.

Per worker replica at defaults (`JOB_THREADS=3`), worst case ≈ **19
connections** (supervisor 1, dispatcher ~2, scheduler ~2 on the queue DB;
uploads pool `JOB_THREADS+2` queue / `JOB_THREADS` primary; default pool 4
queue / 2 primary) — roughly `2×JOB_THREADS + 13`. Web adds ~8. Postgres
17's default `max_connections=100` fits ~3 replicas; the compose file bumps
it to **200** (≈ up to 9 replicas at defaults).

### Compose topology

- **`worker`** (new): `build: .`, `command: ["./bin/jobs"]`, mounts
  `storage:/rails/storage`, env `DATABASE_URL` / `SECRET_KEY_BASE` /
  `RAILS_ENV` / `JOB_THREADS` / `JOB_CONCURRENCY` **plus
  `GED_OIDC_ISSUER`/`GED_OIDC_JWKS_URL`** — `config/initializers/
  oidc_config.rb` raises at boot in production without them, even though the
  worker never validates tokens. No ports, no `container_name` (would block
  `--scale`), no `GED_ADMIN_*` (only `db/seeds.rb` reads them), no
  oauth-provider dependency. `stop_grace_period: 40s`.
  `depends_on: postgres (healthy) + web (healthy)` — `bin/docker-entrypoint`
  runs `db:prepare` only for `./bin/rails server`, so on first boot the
  worker must wait for web to have prepared the schemas.
- **`web`**: drop `SOLID_QUEUE_IN_PUMA`; add healthcheck
  `curl -f http://localhost/up` (curl is in the image, `/up` is routed;
  `start_period` covers the first-boot `db:prepare`).
- **`postgres`**: `command: ["postgres", "-c", "max_connections=200"]`.

### Graceful shutdown

`config/environments/production.rb`:

```ruby
config.solid_queue.shutdown_timeout =
  ENV.fetch("SOLID_QUEUE_SHUTDOWN_TIMEOUT", 30).to_i.seconds
```

The gem default (5s) is thin for a large `StoreDocumentJob` under disk
contention. Compose `stop_grace_period: 40s` > timeout + supervisor
overhead, so scale-downs drain cleanly. Hard kills stay covered by the
chaos-proven at-least-once recovery (supervisor claim release +
`ReconcileUploadRequestsJob`).

### Dev/test parity

- `bin/dev` and `config/puma.rb` unchanged: the `plugin :solid_queue` stays,
  gated by `SOLID_QUEUE_IN_PUMA`, which only `bin/dev` sets now —
  single-process dev convenience is preserved.
- Test env keeps the `:test` adapter; nothing changes.
- `bin/worktree-stack up` already passes extra args to `docker compose up`,
  so `bin/worktree-stack up --scale worker=N` works as-is (usage comment
  updated).

### Runbook

```
# backlog persisting (visible in /admin/upload_requests): add workers, hot
bin/worktree-stack scale 3    # = docker compose up -d --no-build --no-recreate --scale worker=3 worker

# back to normal: surplus replicas drain in-flight jobs, then stop
bin/worktree-stack scale 1
```

`up --scale worker=N` also accepts the flag, but its `--build` recreates
containers whenever the build context changed (any source/docs edit) — the
`scale` subcommand is the guaranteed-hot path.

Pause/Resume from `/admin/upload_requests` is queue-level state in Postgres
— it applies to all replicas at once, unchanged.

### Chaos coverage follows the jobs

`bin/perf-upload --chaos` kills `web`; now that jobs live elsewhere, that no
longer exercises the job-runner's death. New `--chaos-target web|worker`
(default `web`) parametrizes the kill; `worker` runs skip the web
`wait_for_health` (web never goes down).

## Validation

1. `bin/ci` green.
2. Baseline (1 worker): `--profile mix --mode async` and
   `--profile many-small` should match the 2026-07-13 numbers (~2,901 /
   ~6,168 stored docs/min) — moving out of Puma is not a regression.
3. `--scale worker=3`: `stored_per_min` should approach ingest rates
   (7,375 mix / 19,925 many-small); POST p95 not regressed; zero-loss OK.
   `few-large` expected ~flat (disk-bound) — confirms the diagnosis.
4. Zero workers: `docker compose stop worker` → async POSTs still 202 and
   accumulate pending; `start` → drain. Pause/Resume with 3 replicas.
5. Recurring dedupe with 3 replicas: no duplicate `[task_key, run_at]` rows.
6. Chaos: `--chaos` (web) + `--chaos --chaos-target worker`.
7. Report in `docs/perf/2026-07-13-worker-scaling.md` (1 vs 3 workers).

## Non-goals

- **Multi-host / distributed workers** — blocked on object storage + staging
  redesign (see Decision); documented, deferred.
- **Queue-depth autoscaler** — scaling stays a manual `--scale worker=N`.
- **Numeric SLO** — none exists; this adds throughput headroom, not a
  commitment.
- **Kafka / alternative queue backends** — Solid Queue on Postgres remains.
