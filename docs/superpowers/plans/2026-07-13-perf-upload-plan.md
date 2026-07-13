# Perf Upload Tooling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `bin/perf-upload`, a stdlib-only Ruby load driver that measures
sync/async upload throughput against a worktree Docker stack and proves
zero-loss on crash via a `--chaos` scenario; archive campaign reports in
`docs/perf/`.

**Architecture:** One executable script, `bin/perf-upload`, structured as a
`PerfUpload` module with small classes (CLI, Auth, Setup, Generator, Loader,
Poller, Chaos, Report) in a single file — bin/ scripts in this repo are
single-file. It talks only to public surfaces: the HTTP API, the oauth-provider
`/token` endpoint, and `bin/worktree-stack` passthrough (`exec`, `kill`, `up`)
for provisioning and chaos.

**Tech Stack:** Ruby stdlib only — `net/http`, `optparse`, `json`,
`securerandom`, `tempfile`, threads. No Gemfile change.

**Design spec:** `docs/superpowers/specs/2026-07-13-perf-upload-design.md`
(read it first — it fixes profiles, CLI shape, SSRF/callback decision, chaos
protocol).

## Global Constraints

- Stdlib only; no new gem, no app code change.
- `bin/rubocop` must pass (rails-omakase) — the script is Ruby, RuboCop covers `bin/`.
- Generated payload files go to a tempdir, never the repo.
- Every run tags documents `tags[perf_run]=<run-id>` (unique per run).
- Async callback URL is `http://host.docker.internal:9/perf-hook` (see spec —
  fast no-retry refusal by the SSRF guard); verification is polling-based.
- API error contract & pagination: list endpoints max `limit=500` — count
  documents by paging `GET /api/v1/documents?tag=perf_run&tag_value=<run-id>`.
- Commits: only with the user's explicit go-ahead (AGENTS.md).

---

### Task 1: Script skeleton — CLI, profiles, config

**Files:**
- Create: `bin/perf-upload` (executable, `#!/usr/bin/env ruby`)

**Interfaces (produces):**
- `PerfUpload::Options` struct: `profile, mode, count, size_range, threads,
  base_url, token_url, chaos, json_path, setup` — parsed from ARGV.
- `PROFILES = { "mix" => { count: 1_000, sizes: 50_000..5_000_000 }, "many-small" => { count: 10_000, sizes: 4_000..6_000 }, "few-large" => { count: 100, sizes: 20_000_000..100_000_000 } }`
- Stack discovery: derive default `base_url`/`token_url` from
  `bin/worktree-stack urls` output (parse the printed ports) with
  `--base-url` / `PERF_BASE_URL` overrides.

- [ ] Write optparse CLI covering all flags from the spec (`--setup`,
      `--profile`, `--mode`, `--count`, `--size MIN-MAX` accepting `k/m`
      suffixes, `--threads`, `--base-url`, `--chaos`, `--json`).
- [ ] `--help` prints usage; invalid profile/mode aborts with a clear message.
- [ ] Smoke: `bin/perf-upload --help` exits 0; `bin/rubocop bin/perf-upload`
      passes.

### Task 2: Auth + setup subcommand

**Files:**
- Modify: `bin/perf-upload`

**Interfaces:**
- `Auth.token(token_url:, client_id:, client_secret:) -> String` — POST
  `grant_type=client_credentials`, raises on non-200.
- `Setup.run!(options)` — idempotent provisioning:
  1. `bin/worktree-stack exec -T oauth-provider bin/register-client ged-perf`
     → parse `client_secret:` line (re-registering overwrites the secret,
     which is fine/idempotent).
  2. `bin/worktree-stack exec -T web bin/rails runner '<one-liner>'` creating
     `Department.find_or_create_by!(name: "perf-test")`,
     `AppClient.find_or_create_by!(client_identifier: "ged-perf")`, and a
     read+write `ClientDepartmentAccess` (find_or_create, then update both
     flags true); prints `department_id=<id>`.
  3. Print export lines for `PERF_CLIENT_ID/PERF_CLIENT_SECRET/PERF_DEPARTMENT_ID`.
- Runtime config resolution: env vars first; `PERF_DEPARTMENT_ID` fallback =
  runner lookup by department name.

- [ ] Implement `Auth.token` and `Setup.run!` as above.
- [ ] Verify against a live stack: `--setup` twice (idempotent), then a token
      fetch succeeds (assert JWT shape `xxx.yyy.zzz`).

### Task 3: Payload generation + multipart upload client

**Files:**
- Modify: `bin/perf-upload`

**Interfaces:**
- `Generator.files(count:, sizes:, dir:) -> [Pathname]` — random bytes via
  `SecureRandom.bytes` chunks; deterministic names `perf-<i>.bin`.
- `Uploader#post_document(path, async:) -> Result` where
  `Result = Struct(status, latency_s, request_id, document_id, error)`.
  Multipart built by hand (stdlib): boundary + `department_id`, `tags[perf_run]`,
  optional `callback_url`, then the file part streamed
  (`Net::HTTP::Post#body_stream` or assembled body for small files; stream for
  few-large to avoid 100 MB strings in memory).
- Thread pool: N worker threads consuming a `Queue` of paths, collecting
  `Result`s thread-safely.

- [ ] Implement generator + multipart client + pool.
- [ ] Verify live: 10-doc smoke run in sync mode returns 201s; a doc appears
      via `GET /api/v1/documents?tag=perf_run&tag_value=<run-id>`.

### Task 4: Async convergence polling + report + zero-loss verdict

**Files:**
- Modify: `bin/perf-upload`

**Interfaces:**
- `Poller.converge(request_ids, timeout:) -> {id => final_payload}` — polls
  `GET /api/v1/upload_requests/:id` (batched across threads, backoff 0.5→5s)
  until all terminal (`completed`/`failed`) or timeout.
- `Report.render(results, poll_outcomes, api_count)` — accepted docs/min,
  stored docs/min (first-202→last-completed window), POST latency p50/p95/p99
  (sorted-array percentile, no gem), HTTP error tally, zero-loss verdict:
  `202_count == completed_count == api_count`. Stdout table + optional
  `--json` dump (same data, machine shape).
- `ApiCounter.count(tag_value:)` — pages `limit=500&offset=…` until short page.

- [ ] Implement poller, percentiles, report, API counter.
- [ ] Verify live: 20-doc async smoke run converges, verdict green.

### Task 5: Chaos orchestration

**Files:**
- Modify: `bin/perf-upload`

**Interfaces:**
- `Chaos.run!(options)` — async load on a thread; once ≥50% of POSTs got
  their 202 **and** `completed < accepted` (backlog exists):
  1. `bin/worktree-stack kill web` (SIGKILL by default).
  2. Wait for remaining POST threads to finish (they'll record connection
     errors — expected client-visible failures, counted separately).
  3. `bin/worktree-stack up -d web`; wait for `GET /up` (or first 200) health.
  4. Recovery loop: every 30s run `bin/worktree-stack exec -T web bin/rails
     runner 'ReconcileUploadRequestsJob.perform_now'` and poll convergence;
     stop when all 202'd requests are terminal (timeout ~15 min — covers the
     10-min `STUCK_AFTER` guard + Solid Queue claim release on reboot).
  5. Report: split "no 202 (expected failure)" vs "202'd"; verdict green iff
     every 202'd request is `completed` and API count matches.
- Note: `ReconcileUploadRequestsJob`'s `STUCK_AFTER = 10.minutes` means
  requests claimed by the killed process only become reconcilable 10 min
  after their last update; Solid Queue's own claim-release on restart usually
  recovers them much sooner. The loop just waits for whichever wins.

- [ ] Implement chaos flow.
- [ ] Verify live with a small forced run (e.g. `--profile mix --count 100
      --chaos`): kill lands while backlog > 0, recovery completes, verdict green.

### Task 6: Campaigns + reports + docs sync

**Files:**
- Create: `docs/perf/2026-07-13-mix.md`, `docs/perf/2026-07-13-many-small.md`,
  `docs/perf/2026-07-13-few-large.md`, `docs/perf/2026-07-13-chaos.md`
- Modify: `AGENTS.md` (one line in Conventions pointing at `bin/perf-upload`
  and `docs/perf/`)

- [ ] Run: `mix` in `--mode both`, `many-small` async, `few-large` async,
      one `--chaos` run. Archive each report (conditions + stdout table +
      JSON excerpt) in `docs/perf/`.
- [ ] Note the observed bottleneck (expected: single Solid Queue worker,
      `JOB_CONCURRENCY=1`) in the reports — no tuning in this branch.
- [ ] `bin/ci` green (paste real output in the session).
- [ ] Present numbers to the user; explicit confirmation before any merge.
