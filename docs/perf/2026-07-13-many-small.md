# Perf campaign — profile `many-small` (2026-07-13)

Queue/DB/metadata stress: 10 000 documents of ~5 KB, async.

## Conditions

- Command: `bin/perf-upload --profile many-small --mode async` (6 upload threads)
- Stack: isolated worktree Docker stack, production Rails env, Solid Queue in
  Puma (`JOB_CONCURRENCY=1`, 3 worker threads), Thruster in front
- Host: Apple M4 Max, 14 cores, 36 GB RAM; Docker VM 14 CPUs / ~16 GB
- Verification: every 202'd request polled to a terminal state; document
  count re-read from the API filtered on the run's `perf_run` tag

## Results (run perf-20260713-011947-9368e5)

| metric | value |
|---|---|
| POSTs / accepted / errors | 10 000 / 10 000 / 0 |
| POST latency p50 / p95 / p99 / max | 17 / 22 / 26 / 127 ms |
| ingest rate (202 accepted) | **19 925 docs/min** (wall 30.1 s) |
| store rate (jobs completed) | **6 168 docs/min** |
| requests completed / failed | 10 000 / 0 |
| API document count | 10 000 |
| zero-loss | **OK** |

## Reading

- With ~5 KB payloads the 202 path costs 17 ms p50 — the API front accepts
  ~20 000 docs/min, and latencies stay flat through the whole run (p99 26 ms):
  no queue-depth degradation with 10 000 pending jobs in Solid Queue.
- The single Solid Queue worker drains small documents at ~6 200 docs/min
  (~10 ms/job across 3 threads); a 10 000-doc backlog clears in ~1.6 min.
- Zero errors; API count exactly 10 000; zero-loss verdict OK.
