# Observability implementation plan

Spec: `docs/superpowers/specs/2026-07-13-observability-design.md`
Branch: `feat/observability` (worktree `.claude/worktrees/observability`)

Each step: tests first (TDD), `bin/rails test` green before checking the box.

## Part 1 — Admin Status page (ship checkpoint: works with zero new gems)

- [x] 1.1 `Auth::JwksCache#health` — public read-only snapshot
      `{fetched:, age_seconds:, failing:}`; never fetches. Tests in
      `test/services/auth/jwks_cache_test.rb` (or alongside existing cache
      tests): fresh instance, after stubbed fetch, after failed refresh.
- [x] 1.2 `app/services/disk_space.rb` — `DiskSpace.for(path)` via
      `df -Pk`, `Data.define(:total_kb, :available_kb)` + `used_fraction`,
      nil on failure. Tests: real `df` on Rails.root; nil on bogus path.
- [x] 1.3 `app/services/system_health.rb` — checks `database`,
      `job_workers` (SolidQueue::Process heartbeats vs
      `SolidQueue.process_alive_threshold`), `jwks` (from 1.1; never-fetched
      → :unknown, failing → :warn), `storage_disk` (80%/90% thresholds,
      nil → :unknown). Tests in `test/services/system_health_test.rb`.
- [x] 1.4 Route `get "status"` in admin namespace; nav tuple
      `["Status", admin_status_path, "status"]` in
      `app/views/layouts/admin.html.erb`; `Admin::StatusController#index`
      with pipeline counts (UploadRequest by status, ReadyExecution depth for
      uploads/default, Scheduled/Claimed/Failed counts, 5 recent failures,
      UploadQueue.paused?) and docs & storage counts (live/deleted/purgeable,
      blob bytes sum, stored_async split, 24h/7d uploads, top-5 departments
      by blob bytes — two-step, no cross-DB joins).
- [x] 1.5 View `app/views/admin/status/index.html.erb` (health list,
      `.stat-grid` tiles, pause row linking to Upload requests, failures +
      top-departments tables); `AdminHelper#health_badge_class`
      (unknown → info); CSS block in `application.css` (`.section-title`,
      `.stat-grid`, `.stat`, `.health-list/.health-row` — existing tokens
      only).
- [x] 1.6 Tests: `test/controllers/admin/status_controller_test.rb`
      (auth required; 200 with seeded data incl. a
      `SolidQueue::FailedExecution` with JSON error; paused badge; JWKS
      renders "not fetched" with no WebMock stub) + extend
      `test/system/admin_ui_test.rb` (nav → Status, h1, a badge).
      **Checkpoint: run full `bin/rails test` + `bin/rubocop`.**

## Part 2 — OpenTelemetry

- [x] 2.1 Verify current gem versions on rubygems.org, then Gemfile:
      `opentelemetry-metrics-api` (loaded); `require: false`:
      `opentelemetry-sdk`, `opentelemetry-exporter-otlp`,
      `opentelemetry-metrics-sdk` (minor pin), 
      `opentelemetry-exporter-otlp-metrics` (minor pin),
      `opentelemetry-instrumentation-rails`,
      `opentelemetry-instrumentation-pg`,
      `opentelemetry-instrumentation-net_http`. `bundle install`.
- [x] 2.2 `lib/telemetry.rb` facade (meter "ged"; `documents_uploaded`,
      `callback_deliveries`, `job_duration`; cardinality rules in comment).
      `test/lib/telemetry_test.rb`: `assert_not defined?(OpenTelemetry::SDK)`
      + all instruments no-op without raising.
- [x] 2.3 `config/initializers/opentelemetry.rb` gated on
      `OTEL_EXPORTER_OTLP_ENDPOINT` (+ `OTEL_SDK_DISABLED`): requires,
      `SDK.configure` with `use_all` and PG `db_statement: :obfuscate`,
      PeriodicMetricReader (OTLP metrics, 15s) + at_exit shutdown,
      `perform.active_job` → `job_duration`. Comments: Puma-cluster caveat,
      SolidQueue polling-noise escape hatches.
- [x] 2.4 Hooks: `DocumentCreator` (`upload_mode` attr) and
      `DeliverCallbackJob` (refused/success/failure/exhausted) + stub-based
      emission tests. Boot check: `bin/rails runner 1` with and without the
      env var.

## Part 3 — Optional local backend

- [x] 3.1 `docker-compose.yml`: `otel-lgtm` service (pinned image,
      `profiles: ["otel"]`, ports `${GED_GRAFANA_PORT:-3300}:3000` +
      `${GED_OTLP_PORT:-4318}:4318`, dashboards volumes + `lgtm_data`);
      `web` env `OTEL_SERVICE_NAME` + 
      `OTEL_EXPORTER_OTLP_ENDPOINT: ${GED_OTEL_ENDPOINT:-}`; no depends_on.
- [x] 3.2 `bin/worktree-stack`: grafana/otlp ports in offset math (bases
      3300/4318), `GED_OTEL=1` toggle (appends profile + defaults endpoint),
      Grafana URL in `urls()`, header/help comment.
- [x] 3.3 Boot `GED_OTEL=1 bin/worktree-stack up`, generate traffic, verify
      spanmetrics metric names in Prometheus, build + export dashboard →
      `observability/grafana/provisioning/dashboards/ged.yaml` +
      `observability/grafana/dashboards/ged-overview.json`; re-boot to verify
      provisioning mounts against the pinned image.

## Finish

- [x] Docs: README "Observability" section; AGENTS.md (stack, implementation
      map, env vars, conventions).
- [x] Full `bin/ci` green (report actual output).
- [x] `bin/worktree-stack up` (+ `GED_OTEL=1`) for user validation — traces
      in Tempo, `ged_documents_uploaded_total` in Prometheus, dashboard
      renders, `/admin/status` sections live.
- [x] User validates → rebase on main + ff-merge (explicit confirmation
      required), then `bin/worktree-stack clean` + remove worktree.
