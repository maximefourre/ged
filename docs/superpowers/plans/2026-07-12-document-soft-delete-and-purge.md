# Document Soft-Delete + Scheduled Purge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax; each code task is TDD (write failing test → implement → green).

**Goal:** Make document deletion logical (soft) with a 30-day grace window and
admin restore, and add a scheduled job that hard-deletes documents past their
purge date. The async upload-request purge already exists
(`ReconcileUploadRequestsJob`) and is left unchanged.

**Architecture:** `documents` gains `deleted_at` + `purge_at` columns; a model
`default_scope` hides soft-deleted rows everywhere, with `with_deleted` /
`deleted` / `purgeable` escape-hatch scopes and `soft_delete!` / `restore!`.
Both delete endpoints call `soft_delete!`. `PurgeDeletedDocumentsJob`
(`Document.purgeable.find_each(&:destroy!)`, scheduled daily in
`config/recurring.yml`) does the real delete + Active Storage purge. Admins get
a Trash view + restore.

**Tech Stack:** Rails 8.1 (API + ERB admin), PostgreSQL, Solid Queue, Minitest.

**Spec:** `docs/superpowers/specs/2026-07-12-document-soft-delete-and-purge-design.md`

## Global Constraints

- `bin/rubocop` (rails-omakase) must pass before every commit; `bin/ci` runs
  everything (Rails + system tests + brakeman + bundler-audit + seeds).
- Tests are Minitest; **no fixtures** — create records inline (attach a file via
  `doc.file.attach(io: StringIO.new("PDF!"), …)` or `DocumentCreator`/fixtures
  helpers already used in the suite).
- API tests use `JwtTestHelpers` (`stub_jwks!`, `bearer_header("erp-app")`).
- Job tests are `ActiveJob::TestCase`; age rows with `update_columns` and run
  `perform_now`; force async purge with `perform_enqueued_jobs(only: ActiveStorage::PurgeJob)`.
- Migrations inherit `ActiveRecord::Migration[8.1]`.
- Commit messages: repo style (`feat:`/`fix:`/`docs:` + one line), end with
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. **Commit/push/merge
  only on the user's explicit go-ahead.**

---

### Task 1: Migration + `Document` soft-delete model (TDD)

**Files:**
- Create: `db/migrate/<ts>_add_soft_delete_to_documents.rb`
- Modify: `app/models/document.rb`
- Create/Modify: `test/models/document_test.rb`

- [ ] **Step 1: Migration.** `add_column :documents, :deleted_at, :datetime`;
  `add_column :documents, :purge_at, :datetime`; `add_index :documents, :purge_at`.
- [ ] **Step 2: Run it.** `bin/rails db:migrate && bin/rails db:test:prepare`.
  Expected: `schema.rb` shows both columns + the `purge_at` index.
- [ ] **Step 3: Failing model tests.** In `test/models/document_test.rb`:
  `soft_delete!` sets `deleted_at` (~now) and `purge_at` (~now + 30d);
  `default_scope` hides a soft-deleted doc from `Document.all` and makes
  `Document.find(id)` raise `RecordNotFound`; `deleted` and `with_deleted` find
  it; `purgeable` returns it only when `purge_at <= now`; `restore!` clears both
  columns and the doc is visible again.
- [ ] **Step 4: Run → red.** `bin/rails test test/models/document_test.rb`.
- [ ] **Step 5: Implement** the `RETENTION`, `default_scope`, scopes, and
  `soft_delete!` / `restore!` from the spec's "Model behavior" section.
- [ ] **Step 6: Run → green.**

### Task 2: `PurgeDeletedDocumentsJob` + schedule (TDD)

**Files:**
- Create: `app/jobs/purge_deleted_documents_job.rb`
- Create: `test/jobs/purge_deleted_documents_job_test.rb`
- Modify: `config/recurring.yml`

- [ ] **Step 1: Failing job test** (mirror `reconcile_upload_requests_job_test.rb`):
  a soft-deleted doc with `purge_at` back-dated past now is destroyed and its
  blob purged (wrap `perform_now` in `perform_enqueued_jobs(only: ActiveStorage::PurgeJob)`,
  assert `ActiveStorage::Blob` gone); a soft-deleted doc with a future
  `purge_at` and a live doc both survive.
- [ ] **Step 2: Run → red.**
- [ ] **Step 3: Implement** the job (`Document.purgeable.find_each(&:destroy!)`,
  `queue_as :default`, header comment noting the recurring schedule).
- [ ] **Step 4: Run → green.**
- [ ] **Step 5: Schedule.** Add the `purge_deleted_documents` entry under
  `production:` in `config/recurring.yml`.

### Task 3: Delete endpoints → soft-delete (TDD)

**Files:**
- Modify: `app/controllers/api/v1/documents_controller.rb#destroy`
- Modify: `app/controllers/admin/documents_controller.rb#destroy`
- Modify: API + admin destroy tests

- [ ] **Step 1: Update/failing tests.** API `DELETE` → still 204; the row still
  exists via `Document.with_deleted`, has `deleted_at`/`purge_at` set, its
  attachment still exists (`ActiveStorage::Attachment` count unchanged), and a
  follow-up `GET`/`download` returns 404. Admin destroy → soft-deletes (row
  survives, hidden from `/admin/documents`). Keep any asserted flash text in
  sync (see note below).
- [ ] **Step 2: Run → red.**
- [ ] **Step 3: Implement** — swap `destroy!` for `soft_delete!` in both
  actions. If the admin flash changes to "Document moved to trash.", update the
  admin controller test assertion accordingly.
- [ ] **Step 4: Run → green.**

### Task 4: Admin Trash + restore (TDD)

**Files:**
- Create: `app/controllers/admin/deleted_documents_controller.rb`
- Create: `app/views/admin/deleted_documents/index.html.erb`
- Modify: `config/routes.rb` (admin namespace), `app/views/layouts/admin.html.erb` (nav)
- Create: `test/controllers/admin/deleted_documents_controller_test.rb`

- [ ] **Step 1: Failing controller test.** Trash `index` lists a soft-deleted
  doc (and not a live one); `restore` on a soft-deleted doc redirects to the
  Trash index, clears the flags, and the doc reappears in `/admin/documents`;
  the Trash index requires an admin session.
- [ ] **Step 2: Run → red.**
- [ ] **Step 3: Implement** the controller (`index` + `restore`), the route, the
  view (filename / department / deleted-at / purge date + a `button_to`
  Restore, reusing `admin/shared/_tag_chips`), and the "Trash" nav link.
- [ ] **Step 4: Run → green.**

### Task 5: Department deletion guard (TDD, conditional)

**Files:** `app/models/department.rb` (+ its controller/test) — only if admin can delete departments.

- [ ] **Step 1: Verify** whether a department-delete path exists
  (`grep -rn "def destroy" app/controllers/admin/departments_controller.rb`,
  routes). If none, mark N/A and skip.
- [ ] **Step 2 (if it exists): Failing test** — a department with only a
  soft-deleted document cannot be destroyed (no `ForeignKeyViolation`, a
  friendly error instead).
- [ ] **Step 3: Implement** a `before_destroy` on `Department` aborting when
  `Document.with_deleted.where(department_id: id).exists?` (or equivalent), so
  `default_scope` doesn't let `restrict_with_error` be bypassed.
- [ ] **Step 4: Run → green.**

### Task 6: Docs + full CI

**Files:** `AGENTS.md`, optionally `docs/api/openapi.yaml`.

- [ ] **Step 1: AGENTS.md** — document the soft-delete convention
  (`default_scope`, `soft_delete!`/`restore!`, `deleted_at`/`purge_at`), the
  admin Trash/restore, and `PurgeDeletedDocumentsJob` (daily) in "Key
  architectural decisions" / "How the pieces fit". Optionally note the
  soft-delete + 30-day retention in the openapi `DELETE` description.
- [ ] **Step 2: Full CI.** `bin/ci` — all green.
- [ ] **Step 3: Manual verification** (see spec / plan file Verification).
- [ ] **Step 4: Commit** on the user's go-ahead; get confirmation before the
  final merge into `main`.
