# Perf campaign — worker scaling, 1 vs 3 replicas (2026-07-13)

Validation of the dedicated `worker` compose service (Solid Queue extracted
from Puma — see `docs/superpowers/specs/2026-07-13-worker-scaling-design.md`):
does moving jobs out of Puma regress anything, and what does hot-scaling to
3 replicas buy?

## Conditions

- Commands: `bin/perf-upload --profile mix|many-small|few-large --mode async`
  (6 upload threads), replica count switched hot via `bin/worktree-stack
  scale 1|3` between runs
- Stack: isolated worktree Docker stack, production Rails env, **dedicated
  `worker` service** (`./bin/jobs`; per replica: `uploads` pool
  `JOB_THREADS=3` × `JOB_CONCURRENCY=1` + `default` pool 2 threads), web
  healthcheck-gated, Postgres `max_connections=200`, Thruster in front of
  Puma (web no longer runs Solid Queue — verified 0 supervisors in its logs)
- Host: Apple M4 Max, 14 cores, 36 GB RAM; Docker VM 14 CPUs / ~16 GB
- Verification: every 202'd request polled to a terminal state; document
  count re-read from the API filtered on the run's `perf_run` tag

## Results

### profile `mix` (1,000 docs, 50 KB – 5 MB, async)

| metric | 1 worker (180623-e86ded) | 3 workers (181047-cf238d) |
|---|---|---|
| POSTs / accepted / errors | 1 000 / 1 000 / 0 | 1 000 / 1 000 / 0 |
| POST latency p50 / p95 / p99 / max | 38 / 87 / 121 / 207 ms | 45 / 99 / 152 / 275 ms |
| ingest rate (202 accepted) | 7 932 docs/min | 6 666 docs/min |
| store rate (jobs completed) | **3 105 docs/min** | **5 580 docs/min (+80 %)** |
| zero-loss / API count | OK / 1 000 | OK / 1 000 |

### profile `many-small` (10,000 docs, ~5 KB, async)

| metric | 1 worker (180653-eee996) | 3 workers (181116-b9268d) |
|---|---|---|
| POSTs / accepted / errors | 10 000 / 10 000 / 0 | 10 000 / 10 000 / 0 |
| POST latency p50 / p95 / p99 / max | 17 / 22 / 26 / 55 ms | 22 / 29 / 38 / 298 ms |
| ingest rate | 19 538 docs/min | 15 310 docs/min |
| store rate | **6 674 docs/min** | **10 735 docs/min (+61 %)** |
| zero-loss / API count | OK / 10 000 | OK / 10 000 |

### profile `few-large` (100 docs, 20 – 100 MB, async)

| metric | 1 worker (181304-6882bf) | 3 workers (181229-aa01c0) |
|---|---|---|
| POSTs / accepted / errors | 100 / 100 / 0 | 100 / 100 / 0 |
| POST latency p50 / p95 / p99 / max | 492 / 908 / 951 / 1 520 ms | 432 / 773 / 894 / 925 ms |
| ingest rate | 632 docs/min | 722 docs/min |
| store rate | **264 docs/min** | **630 docs/min (+139 %)** |
| zero-loss / API count | OK / 100 | OK / 100 |

## Reading

- **No regression from the extraction**: at 1 worker, store rates match the
  in-Puma baselines of the 2026-07-13 campaigns within noise (mix 3 105 vs
  2 901; many-small 6 674 vs 6 168; few-large 264 vs 279 docs/min).
- **Scaling works**: 3 replicas buy +80 % (mix), +61 % (many-small),
  +139 % (few-large); store rates now approach ingest rates, so the backlog
  drains roughly as fast as it builds.
- **The few-large "disk-bound" reading from the earlier campaign was wrong**:
  the ~280 MB/s ceiling was the *single worker process*, not the SSD —
  3 processes move ~630 MB/s (~3 × 264 = 792 in theory; contention accounts
  for the gap). On slower disks the flattening will come earlier.
- Ingest dips 15–20 % while 3 replicas drain concurrently (same-host CPU
  contention); net wall-clock is still much better because storing overlaps
  ingesting. p95 POST latency stays within a few tens of ms.

## Operational checks (3 replicas unless noted)

- **Hot scale**: `bin/worktree-stack scale 1` → `scale 3` adds/removes only
  worker containers — web untouched (uptime preserved), no rebuild. (This is
  why the `scale` subcommand exists: `up --build --scale` recreates
  containers whenever the build context changed.)
- **Zero workers**: with all workers stopped, 20 async POSTs → all 202,
  all `pending`; drained to `completed` <10 s after `start worker`.
- **Pause/Resume** (`UploadQueue` facade, same as the admin button): 10
  POSTs while paused stayed `pending` across all 3 replicas; all completed
  ~6 s after resume — the pause is queue-level state in Postgres, replicas
  all honor it.
- **Recurring dedupe**: 2 scheduler ticks fired with 3 schedulers running —
  each `[task_key, run_at]` exactly once, duplicate query returns 0.

## Chaos

### `--chaos` (kill web, 3 workers active — run 181540-29dd6b)

- 519/1,000 accepted before SIGKILL; web back **3 s** after kill.
- 546 requests had been accepted in total; **546/546 completed, zero loss**
  (the 454 POST "errors" are connections refused while web was down —
  never 202'd, so nothing was owed). Workers kept draining the backlog
  while web was down; recovery converged immediately.

### `--chaos --chaos-target worker` (kill all 3 replicas at 500/1,000 accepted — run 181634-cef1dc)

- **Web never noticed**: 1,000/1,000 POSTs accepted, 0 errors (p95 95 ms) —
  ingest is fully decoupled from the workers' death.
- After the restart the backlog drained except **6 stragglers** whose jobs'
  claims died with the killed supervisors; `ReconcileUploadRequestsJob`
  re-enqueued all 6 at the 10-min `STUCK_AFTER` guard (16:26:54Z) and they
  completed immediately — full convergence ≈ 11 min after the kill.
- **Zero documents lost, 2 duplicates**: all 1,000 requests completed; the
  API counts 1,002 documents = 1,000 distinct filenames + 2 filenames
  stored twice (`…-605.bin`, `…-606.bin` — killed between committing the
  document and marking the request completed). That was the documented
  **at-least-once** recovery semantics (same as the 2026-07-13 soak, which
  saw 1 duplicate/613). `perf-upload`'s strict `count == POSTs` check
  therefore prints `zero-loss: FAILED` on duplicates; actual loss would be
  a count *below* 1,000.
- The run's `stored docs/min` figure (97) is meaningless — `store_seconds`
  spans the 10-min reconcile tail, not steady-state throughput.

### Duplicate window closed (same branch, follow-up)

`StoreDocumentJob` now commits the document INSERT and the request's
completed UPDATE **in a single transaction**: an aborted run persists
nothing, so a reconcile re-run stores the document exactly once (Active
Storage defers the blob's disk write to commit time, so a rollback leaves
no orphan file either). Proven by a deterministic unit test that crashes
the job between the two writes (`test/jobs/store_document_job_test.rb`).
Document recovery is now **exactly-once**; job runs and callbacks remain
at-least-once. A post-fix `--chaos-target worker` rerun is recorded below.

### `--chaos --chaos-target worker` rerun after the fix (run 225913-fac398)

- 1,000/1,000 POSTs accepted, 0 errors (p95 98 ms); all 3 replicas
  SIGKILLed at 500 accepted, rebuilt-image stack.
- **1,000/1,000 requests completed and exactly 1,000 documents (1,000
  distinct filenames verified in SQL) — zero loss, zero duplicates,
  `zero-loss: OK`.** Stragglers whose claims died with the killed
  supervisors were re-enqueued by the reconcile guard and converged, same
  recovery path as before — this time without re-storing anything. One
  clean run is consistent with the fix, not a proof — the deterministic
  unit test is the proof; the rerun checks nothing regressed end-to-end.
