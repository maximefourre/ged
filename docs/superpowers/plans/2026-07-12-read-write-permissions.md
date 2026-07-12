# Read/Write Permissions + SSRF Callback Guard — Implementation Plan

**Goal:** Split each client's per-department access into independent `can_read` /
`can_write` grants (upload-but-not-read, or read-but-not-upload), and fix the
async-callback SSRF by filtering non-routable `callback_url` targets.

**Spec:** `docs/superpowers/specs/2026-07-12-read-write-permissions.md`
**Audit:** `docs/security/audit-2026-07-12.md`

**Tech stack:** Rails 8.1 (API + ERB admin), PostgreSQL, Minitest + WebMock.

## Global constraints

- Work happens in the `feat/read-write-permissions` worktree; never on `main`.
- `bin/rubocop` (rails-omakase) must pass before every commit; `bin/rails test`
  green (system tests via `bin/rails test:system`; `bin/ci` runs everything).
- Tests are Minitest; **no fixtures** — create records inline in `setup`.
- API tests use `JwtTestHelpers` (`stub_jwks!`, `bearer_header("<client>")`);
  `Auth::JwksCache.reset!` runs automatically.
- WebMock: `assert_requested(:post, url) { |req| … }` (block-arg form).
- Migrations inherit `ActiveRecord::Migration[8.1]`.
- API error contract: 401 invalid/missing token, 403 forbidden, 404 not found,
  400 bad param, 422 validation.

---

### Task 1 — Migration: read/write columns

- [x] `db/migrate/*_add_read_write_to_client_department_accesses.rb`: add
  `can_read` / `can_write` (boolean, null:false, default:true); backfill is the
  default. Add check constraint `at_least_one_permission` (`can_read OR
  can_write`). Keep the unique `[app_client_id, department_id]` index.
- [x] `bin/rails db:migrate` → schema.rb updated.

### Task 2 — Models (TDD)

- [x] `test/models/client_department_access_test.rb`: `at_least_one_permission`
  rejects both-false.
- [x] `test/models/app_client_test.rb`: `can_access?(dept, :read/:write)` +
  mixed cascade (parent RW, child R-only).
- [x] `ClientDepartmentAccess#at_least_one_permission` validation.
- [x] `AppClient#can_access?(department, mode)` (required mode),
  `accessible_department_ids(mode = :read)`, private `granted_department_ids(mode)`.

### Task 3 — Authorization wiring (TDD)

- [x] `Api::V1::BaseController#authorize_department!(department, mode:)` (required
  kwarg; message `"no #{mode} access to this department"`).
- [x] `find_authorized_document(mode:)`; `show`/`download` → `:read`,
  `destroy` → `:write`.
- [x] Update every call site: documents index scope+filter (read), create
  (write); departments index (read), create (write); document_tags update
  (write); upload_requests show (read).
- [x] Controller tests: read-only → 403 on create/destroy/tags; write-only → 403
  on index/show/download; destroy needs write.

### Task 4 — SSRF callback guard (TDD)

- [x] `test/services/callback_url_policy_test.rb`: block 127/8, ::1,
  169.254.169.254, 10/8, 192.168/16, `::ffff:127.0.0.1`, DNS→private (stub
  `Resolv`); allow public.
- [x] `app/services/callback_url_policy.rb`: `reject_reason(url, resolve:)`.
- [x] `UploadRequest`: replace `callback_url_is_http` with
  `callback_url_allowed` (no DNS) → 422.
- [x] `DeliverCallbackJob`: guard at top of `perform` (`resolve: true`) → log +
  return, no retry. `test/jobs/deliver_callback_job_test.rb`: private target ⇒ no
  POST; public ⇒ POST.
- [x] Controller test: async `callback_url=http://127.0.0.1` → 422.

### Task 5 — Admin UI grid (TDD)

- [x] `Admin::AppClientsController`: strong params drop `department_ids`;
  `access_params` + `sync_access!` reconciliation (transactional); load
  `@departments`; `includes(client_department_accesses: :department)`.
- [x] Views: `_form` → per-department read/write checkbox grid (pre-checked);
  `index`/`show` → R/W badges.
- [x] `test/controllers/admin/app_clients_controller_test.rb`: RW / read-only /
  write-only create; unchecking removes the row.
- [x] `test/system/admin_ui_test.rb`: update the app-client flow to the grid.

### Task 6 — Docs + CI

- [x] `docs/api/openapi.yaml`: read/write descriptions + `callback_url` public.
- [x] `AGENTS.md`: permission-model + authorization implementation lines.
- [ ] `bin/ci` green (report real output).
- [ ] `bin/worktree-stack up` + `seed` → isolated stack; present to user for
  validation **before** any merge. Ménage (`bin/worktree-stack clean` + remove
  worktree) on repatriation.
