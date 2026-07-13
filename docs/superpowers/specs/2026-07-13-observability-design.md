# Observability: OpenTelemetry + Admin Status Page — Design

Date: 2026-07-13
Status: approved (brainstormed and validated with the user)

## Problem

GED has zero observability. There are no metrics or tracing gems, no
`ActiveSupport::Notifications` subscribers, no structured logging, and the
admin UI has no operational view — `Admin::DashboardController#index` is a
placeholder redirect to Documents. An operator cannot answer basic questions
(is the upload pipeline healthy? how much is stored? are callbacks failing?)
without a Rails console, and there is no request/job tracing at all.

## Goals

1. A **Status page in the admin UI** showing key metrics with zero external
   dependencies — usable on any deployment, always.
2. **OpenTelemetry instrumentation** (traces + a few custom metrics) exported
   over OTLP, so GED plugs into any OTel backend — including the company's
   future observability infrastructure.
3. An **optional local backend** for development and self-hosted use, with a
   ready-made Grafana dashboard.

## Non-goals

- OTel logs bridge (experimental in Ruby; revisit later).
- Alerting/paging (Grafana can be configured by hand; not provisioned here).
- Public/unauthenticated status page — the Status page is admin-only.
  (`/up` remains the unauthenticated liveness probe.)
- Per-request custom metrics for HTTP latency in Ruby — derived from spans in
  the backend instead (the Ruby instrumentation gems emit traces only).

## Design

Hybrid, three parts. Part 1 is independently shippable.

### Part 1 — Admin Status page (`GET /admin/status`)

New `Admin::StatusController` + "Status" nav entry. Admin root and the
dashboard redirect are untouched (system-test helpers depend on them).
Server-rendered ERB, no JS/charts; stat tiles + tables + badges on the
existing CSS tokens. Three sections:

- **System health** (green/orange/red checks, all local — no live HTTP on
  page load):
  - Database: `SELECT 1`.
  - Job workers: `SolidQueue::Process` rows of kind Worker with a heartbeat
    newer than `SolidQueue.process_alive_threshold`.
  - OIDC JWKS: a new read-only `Auth::JwksCache#health` snapshot
    (`fetched` / `age_seconds` / `failing`). Never-fetched renders neutral
    ("no API traffic yet"), fetched-but-failing renders warn (serving stale
    keys). Staleness alone is normal — the cache refreshes lazily.
  - Storage disk: `df -Pk` on the Active Storage root (Ruby has no stdlib
    statvfs; a native gem is overkill). ≥80% used → warn, ≥90% → bad,
    `df` failure → unknown.
- **Upload pipeline**: upload requests by status, Solid Queue ready depth per
  queue (`uploads`, `default`), scheduled/in-flight/failed job counts, the 5
  most recent failed executions, and the uploads-queue pause state (link to
  the existing pause/resume controls on Upload requests — not duplicated).
- **Documents & storage**: live/trash/purgeable counts, total blob bytes,
  sync/async split, uploads in the last 24h/7d, top-5 departments by stored
  volume.

Services: `SystemHealth` (returns `Check = Data.define(:name, :state,
:detail)`) and `DiskSpace` (nil-safe `df` wrapper) — both plain classes under
`app/services/`, unit-tested. Counting queries live in the controller,
following the `Admin::UploadRequestsController` precedent. SolidQueue models
are never SQL-joined to app tables (separate queue database).

### Part 2 — OpenTelemetry instrumentation

- **Traces**: auto-instrumentation via `opentelemetry-instrumentation-rails`
  (meta-gem) + `pg` (with `db_statement: :obfuscate` — tag/filename values
  must never ship in spans) + `net_http` (covers callback POSTs and JWKS
  fetches).
- **Custom metrics** (experimental Ruby metrics SDK, minor-pinned):
  - `ged.documents.uploaded` counter, attribute `upload_mode` (sync/async);
  - `ged.callbacks.deliveries` counter, attribute `outcome`
    (success/failure/refused/exhausted), counted per attempt;
  - `ged.job.duration` histogram from a `perform.active_job` subscription,
    attributes job class + queue.
  Cardinality discipline: attribute values only from closed sets — never
  filenames, tags, departments, or URLs.
- **Facade**: `lib/telemetry.rb` memoizes the meter and instruments. Only
  `opentelemetry-metrics-api` loads at boot; its instruments are free no-ops
  without an SDK, so call sites are unconditional.
- **Gating**: `config/initializers/opentelemetry.rb` does nothing unless
  `OTEL_EXPORTER_OTLP_ENDPOINT` is set (and `OTEL_SDK_DISABLED` isn't
  "true"). All other OTel gems are `require: false`. Dev/test and
  deployments without a backend pay nothing, and the WebMock-hermetic test
  suite stays hermetic. `SOLID_QUEUE_IN_PUMA` means this one init covers web
  requests and jobs alike.

### Part 3 — Optional local backend

- `grafana/otel-lgtm` all-in-one container (OTel collector + Prometheus +
  Tempo + Loki + Grafana) as a compose service under `profiles: ["otel"]`.
- Host ports: Grafana `${GED_GRAFANA_PORT:-3300}` (base 3300 — worktree web
  offsets occupy 3000–3250), OTLP `${GED_OTLP_PORT:-4318}` (published so
  native `bin/dev` can export too). `bin/worktree-stack` adds both to its
  offset math and gains a `GED_OTEL=1` convenience toggle.
- `web` always carries `OTEL_EXPORTER_OTLP_ENDPOINT: ${GED_OTEL_ENDPOINT:-}`;
  empty default = SDK disabled, so no `depends_on` and no conditional-env
  hacks. Turning it on = two `.env` lines (`COMPOSE_PROFILES=otel`,
  `GED_OTEL_ENDPOINT=http://otel-lgtm:4318`).
- A Grafana dashboard is provisioned from the repo
  (`observability/grafana/…`): API rate/p95/error-ratio from Tempo
  metrics-generator span metrics, plus the custom metrics (Prometheus
  name-mangled: `ged_documents_uploaded_total`, …).

## Alternatives considered

- **Full permanent Grafana stack**: rejected — too heavy for a 10-person
  internal tool, and per-worktree stacks would balloon.
- **Admin page only, no OTel**: rejected — no latency/error/trace visibility,
  doesn't meet the request.
- **Traces-only / enriched-spans instead of custom metrics**: viable, but the
  user chose traces + metrics; the experimental metrics SDK risk is contained
  by minor-level pins and the no-op fallback.

## Error handling

- Every health check catches its own errors and degrades to a bad/unknown
  badge — the Status page never 500s because a subsystem is down.
- `DiskSpace.for` returns nil on any failure (missing `df`, bad path).
- OTel export failures are the SDK's problem (logged, buffered, dropped) —
  never user-facing; the app runs identically with the backend absent.

## Testing

- Unit: `SystemHealth` per-check states, `DiskSpace` (real `df` + failure),
  `JwksCache#health` snapshots, `Telemetry` no-op path plus a
  `assert_not defined?(OpenTelemetry::SDK)` guard proving the initializer
  stays dormant in test.
- Controller: `/admin/status` auth + rendering with seeded data (incl. a
  `SolidQueue::FailedExecution`), pause badge, and the JWKS check rendering
  without any WebMock stub (proving no HTTP on page load).
- Emission: stub the `Telemetry` instruments in `DocumentCreator` and
  `DeliverCallbackJob` tests (one assertion per outcome).
- System: nav → Status renders with health badges.
- No `openapi.yaml` change (admin HTML page, nothing under `/api/v1`).
