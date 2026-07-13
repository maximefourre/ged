# Perf & resilience tooling for bulk document uploads — Design

**Date:** 2026-07-13
**Status:** approved (brainstormed + validated in session)

## Problem

Two questions the repo cannot answer today:

1. **Throughput** — how many documents/minute does GED ingest in sync and
   async mode, at what latencies, and what is the actual bottleneck?
2. **Resilience** — if the app (Puma, which also hosts Solid Queue via
   `SOLID_QUEUE_IN_PUMA`) crashes mid-load, are queued documents lost?

Code analysis says "no loss": the async pipeline persists the
`UploadRequest` row (Postgres), the raw bytes (`storage/staging/<id>`),
and the job (Solid Queue, Postgres-backed) **before** returning 202;
recovery comes from Solid Queue releasing claimed executions of dead
processes plus `ReconcileUploadRequestsJob` (every 10 min in production)
re-enqueueing stuck `pending`/`processing` requests while the staging file
exists. This design adds tooling to **prove it empirically** and to measure
throughput, repeatably.

## Decisions

- Measure throughput **and** run a chaos scenario (kill the web container
  mid-load, verify zero loss).
- Reusable, committed tooling — not a one-shot campaign.
- Three predefined volume profiles.
- Plain-Ruby script, stdlib only (`net/http`, `json`, `optparse`, threads) —
  no new dependency, same language as the repo.

## Deliverable

`bin/perf-upload`, run from a worktree against that worktree's isolated
Docker stack (`bin/worktree-stack up`). The Docker stack is required anyway:
the chaos scenario kills the `web` container.

### CLI

```
bin/perf-upload --setup
bin/perf-upload --profile mix|many-small|few-large \
                [--mode sync|async|both] [--count N] [--size MIN-MAX] \
                [--threads N] [--base-url URL] [--chaos] [--json FILE]
```

Profiles (overridable with `--count` / `--size`):

| profile      | count  | sizes        | stresses                          |
|--------------|--------|--------------|-----------------------------------|
| `mix`        | 1 000  | 50 KB – 5 MB | realistic reference load          |
| `many-small` | 10 000 | ~5 KB        | queue, DB, metadata               |
| `few-large`  | 100    | 20 – 100 MB  | staging disk, Active Storage, job duration |

### Setup (`--setup`, once per stack)

Provisions everything through the worktree stack:

1. `bin/worktree-stack exec oauth-provider bin/register-client ged-perf`
   → client credentials (SQLite store in the provider).
2. `bin/worktree-stack exec web bin/rails runner ...` → `Department`
   "perf-test", `AppClient client_identifier: "ged-perf"`, and a
   `ClientDepartmentAccess` with `can_read: true, can_write: true`.
3. Prints the credentials + department id; subsequent runs read
   `PERF_CLIENT_ID` / `PERF_CLIENT_SECRET` / `PERF_DEPARTMENT_ID` (or
   re-discover the department by name via the runner).

The script obtains a JWT per run via `POST /token`
(`grant_type=client_credentials`) on the stack's oauth-provider port.

### Run phases

1. **Generate** random-byte files in a tempdir (never in the repo), sizes
   drawn from the profile.
2. **Load**: a thread pool (default 6 ≈ 2× `RAILS_MAX_THREADS`) POSTs
   `multipart/form-data` to `/api/v1/documents` with
   `department_id`, `file`, `tags[perf_run]=<run-id>`, and — in async mode —
   a `callback_url`. Each document of a run carries the same unique
   `perf_run` tag so results are countable and attributable.
3. **Converge** (async): poll `GET /api/v1/upload_requests/:id` until every
   request is terminal, with a global timeout.
4. **Report**: accepted docs/min, stored docs/min (async), POST latency
   p50/p95/p99, HTTP error counts, and the **zero-loss verdict**:
   every 202'd request `completed` + the API document count for
   `?tag=perf_run&tag_value=<run-id>` matches. Table on stdout, `--json`
   for archiving.

### Callback URL under the SSRF guard

`callback_url` is what *triggers* async mode, but `CallbackUrlPolicy`
blocks every address reachable from inside the compose network (RFC1918
et al.), so a real callback receiver is impossible without loosening the
policy — out of scope. The script therefore verifies via **polling** and
passes `http://host.docker.internal:9/perf-hook` as callback: it passes the
syntactic request-time check (hostname, no DNS), and at delivery time it
resolves to an RFC1918 address → the job refuses immediately, **no retry**,
so callback noise doesn't skew the measurement. (An unresolvable hostname
would be worse: 5 retried delivery attempts per document.)

### Chaos scenario (`--chaos`, async only)

1. Start the load; once a target share of 202s is in and jobs are being
   worked, `docker kill` (SIGKILL — `StoreDocumentJob`'s `ensure` must not
   run) the stack's `web` container.
2. `bin/worktree-stack up -d web` to restart.
3. Accelerate recovery instead of waiting for the 10-min cycle:
   `bin/worktree-stack exec web bin/rails runner
   'ReconcileUploadRequestsJob.perform_now'` (after the Solid Queue
   process-alive threshold, ~5 min, or immediately — reconcile's own
   10-minute `STUCK_AFTER` guard is the limiting factor; the script may
   shorten waiting by retrying reconcile until counts stop moving).
4. Verify: every request that got a 202 ends `completed`, and the API
   document count for the run's tag matches. In-flight POSTs killed
   **before** their 202 are expected client-visible failures (correct
   behavior — the client never got a `request_id`), not data loss; the
   report counts them separately.

## Non-goals

- No k6 / external load tool, no CI integration, no Kafka.
- No app/code changes. If campaigns reveal a bottleneck (expected:
  `JOB_CONCURRENCY=1`), the report documents it; tuning is a follow-up
  decision.
- No real callback-delivery load test (see SSRF note above).

## Documentation

- Campaign reports in `docs/perf/<date>-<profile>.md` (numbers + run
  conditions: profile, concurrency, machine).
- One line in `AGENTS.md` pointing to `bin/perf-upload`.
