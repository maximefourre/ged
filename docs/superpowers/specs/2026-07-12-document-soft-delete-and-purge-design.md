# Document Soft-Delete + Scheduled Purge — Design

**Date:** 2026-07-12
**Status:** Approved (design validated in brainstorming session)

## Context

Deleting a document today is destructive and immediate: both
`DELETE /api/v1/documents/:id` and the admin delete call `Document#destroy!`,
which hard-deletes the row and (via Active Storage's default
`has_one_attached` `purge_later`) asynchronously destroys the file — with no
recovery path.

Deletion should instead become **logical (soft) delete with a 30-day grace
window**: marking a document deleted hides it from every read path but keeps
the bytes, records a **purge date** (`deleted_at + 30 days`), and lets an admin
**restore** it during the window. A scheduled job then hard-deletes documents
whose purge date has passed. The user's rationale for restore: without it, the
30-day retention has no purpose.

The parallel ask — a **purge system for async upload requests** — already
exists and is kept as-is: `ReconcileUploadRequestsJob` hard-deletes
`completed`/`failed` `upload_requests` and their staging files 30 days after
creation, every 10 min in production. This design covers only the document
side.

## Goals

- `DELETE /api/v1/documents/:id` and the admin delete soft-delete instead of
  hard-deleting: the document disappears from all reads, its file is retained.
- A `purge_at` date (`deleted_at + 30.days`) is recorded at deletion time.
- Admins can see soft-deleted documents in a Trash view and **restore** them
  before `purge_at`.
- A scheduled job hard-deletes documents past `purge_at` (row + file).

## Non-goals

- No change to the async upload-request purge (already implemented).
- No API surface for restoring or listing deleted documents — restore is an
  admin-only action; to an API client a soft-deleted document is simply 404.
- No new document delete/restore audit log, no configurable retention per
  document, no "empty trash now" admin button (purge is time-based only).
- No full-text search, versioning, backups (unchanged from the main spec).

---

## Data model

Two nullable columns on `documents` (real columns, **not** jsonb `tags`, which
are client-facing and filterable):

- `deleted_at` (`datetime`, null) — the soft-delete marker/moment. `NULL` = live.
- `purge_at` (`datetime`, null) — the scheduled hard-delete date
  (`deleted_at + 30.days`). Indexed (the purge job filters on it).

Keeping both (rather than `purge_at` alone) records the original deletion time
independently of the purge date, so the retention window can change later
without losing when the document was deleted.

**Migration** (`ActiveRecord::Migration[8.1]`, `def change`): `add_column`
`deleted_at` and `purge_at`; `add_index :documents, :purge_at`.

## Model behavior — `Document`

```ruby
RETENTION = 30.days

default_scope { where(deleted_at: nil) }
scope :with_deleted, -> { unscope(where: :deleted_at) }
scope :deleted,      -> { with_deleted.where.not(deleted_at: nil) }
scope :purgeable,    -> { with_deleted.where(purge_at: ..Time.current) }

def soft_delete!
  now = Time.current
  update!(deleted_at: now, purge_at: now + RETENTION)
end

def restore!
  update!(deleted_at: nil, purge_at: nil)
end
```

- `default_scope` hides soft-deleted documents from **every** query path (API
  index/show/download, admin index, all `Document.find` calls) at a single
  choke point. For a permission-scoped store this trades one known
  `default_scope` footgun — association traversal (e.g. `department.documents`)
  also hides soft-deleted rows — for zero risk of ever leaking a "deleted"
  document to an API client through a query site someone forgot to filter.
- `with_deleted` (`unscope`) is the escape hatch the purge job and Trash view
  use to see hidden rows. `purgeable` selects only rows with a past `purge_at`
  (NULLs are excluded by the range comparison, so it implies soft-deleted).
- Range style `where(purge_at: ..Time.current)` matches the app idiom
  (`...RETENTION.ago` in `ReconcileUploadRequestsJob`).

## Delete endpoints → soft-delete

| Endpoint | Change |
| --- | --- |
| `DELETE /api/v1/documents/:id` | `document.soft_delete!` instead of `destroy!`; still `204 No Content`. `find_authorized_document` → `Document.find` respects `default_scope`, so a repeat delete / a `GET` / a `download` of an already-soft-deleted doc returns **404** — to the client it is gone. The file is **not** purged. |
| Admin `DELETE /admin/documents/:id` | `Document.find(params[:id]).soft_delete!`; redirect to the documents index. |

## Purge job + schedule

`PurgeDeletedDocumentsJob` (modeled on `ReconcileUploadRequestsJob`):

```ruby
# Hard-deletes documents whose soft-delete retention window has elapsed.
# Scheduled via Solid Queue (config/recurring.yml).
class PurgeDeletedDocumentsJob < ApplicationJob
  queue_as :default

  def perform
    Document.purgeable.find_each(&:destroy!)
  end
end
```

`destroy!` on a purgeable doc does the real hard delete and fires Active
Storage's default async blob/file purge — no explicit `file.purge` needed. The
`upload_requests.document_id` FK (`on_delete: :nullify`) cleans up any link.

Scheduled daily in `config/recurring.yml` under `production:` (daily is ample
for a 30-day window; Fugit natural-language, matching the existing entries):

```yaml
  purge_deleted_documents:
    class: PurgeDeletedDocumentsJob
    schedule: every day at 3am
```

Like the other recurring tasks it runs only where the Solid Queue supervisor
runs (production / `bin/dev`); the job class is unit-tested directly.

## Admin Trash + restore

- **`Admin::DeletedDocumentsController`** (`< Admin::BaseController`):
  - `index` → `paginate(Document.deleted.includes(:department).order(deleted_at: :desc))`.
  - `restore` → `Document.deleted.find(params[:id]).restore!`; redirect to the
    Trash index with a `"Document restored."` notice.
- **View** `app/views/admin/deleted_documents/index.html.erb`: a table of
  filename / department / deleted-at / purge date, reusing
  `admin/shared/_tag_chips` for tags and the existing admin table CSS classes;
  a `button_to` Restore per row.
- **Route** (admin namespace):

  ```ruby
  resources :deleted_documents, only: [ :index ] do
    patch :restore, on: :member
  end
  ```
- **Nav**: a "Trash" link in the admin top-bar (`app/views/layouts/admin.html.erb`,
  alongside Documents / Departments).

## Error contract (unchanged pattern)

- API delete stays `204`; subsequent reads of a soft-deleted document are `404`
  (default_scope), matching "not found".
- Restore and Trash index require an admin session (existing Devise guard).

## Testing

- **Model**: `soft_delete!` sets `deleted_at`/`purge_at` (`= +30d`); `restore!`
  clears them; `default_scope` hides soft-deleted from default reads and
  `find`; `deleted` / `purgeable` / `with_deleted` scopes select correctly.
- **Job**: age `purge_at` past now (`update_columns`), assert the doc is
  destroyed and its blob purged (`perform_enqueued_jobs(only: ActiveStorage::PurgeJob)`);
  a not-yet-due soft-deleted doc and a live doc are both kept. Mirrors
  `test/jobs/reconcile_upload_requests_job_test.rb`.
- **API**: `DELETE` soft-deletes — the row survives with `deleted_at`/`purge_at`
  set, the file is not purged, and a subsequent `GET`/`download` returns 404.
- **Admin**: delete soft-deletes (row survives, hidden from the index); Trash
  index lists soft-deleted docs; `restore` clears the flags and the doc
  reappears in the normal index.
- **System** (optional): delete → appears in Trash → Restore → back in
  Documents (match existing Capybara patterns/labels).

## Documentation

- Update `AGENTS.md`: the soft-delete convention (`default_scope`,
  `soft_delete!`/`restore!`, `deleted_at`/`purge_at`), the admin Trash/restore,
  and `PurgeDeletedDocumentsJob` (scheduled daily) in "Key architectural
  decisions" / "How the pieces fit".
- `docs/api/openapi.yaml`: no structural change (the DELETE path is unchanged,
  so `api_docs_test` still passes); optionally note the soft-delete + 30-day
  retention in the DELETE description.

## Known considerations

- **Department deletion vs. `restrict_with_error`:** `Department has_many
  :documents, dependent: :restrict_with_error` checks the association, which now
  respects `default_scope` — a department whose only documents are soft-deleted
  would no longer be blocked, and the DB FK (`documents.department_id`, no
  `on_delete`) would then raise on delete. If admin can delete departments, add
  a guard (a `before_destroy` counting `Document.with_deleted` for the
  department) so soft-deleted docs still block deletion. The DB FK is the
  backstop against corruption regardless.
- `find_each` + `default_scope` is safe here (no default ordering to conflict
  with); `purgeable` uses `with_deleted` so batching sees hidden rows.
