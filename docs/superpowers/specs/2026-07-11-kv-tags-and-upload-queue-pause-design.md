# KV Tags + Upload Queue Pause — Design

**Date:** 2026-07-11
**Status:** Approved (design validated in brainstorming session)

## Context

Two requests drive this design:

1. The async upload callback should return the document's tags as a key→value
   map. Today tags are flat names attached *after* creation via
   `POST /api/v1/documents/:id/tags`, so an async-created document never has
   tags at callback time — returning tags only makes sense if tags can also be
   supplied at upload. The user explicitly chose **real key→value tags**
   (à la S3 object tags / K8s labels) stored as a **jsonb map on documents**,
   replacing the flat tag registry.
2. Admins need to pause consumption of the async upload queue. The
   `/admin/upload_requests` index already exists (status filter + pagination);
   what's missing is the pause. Required semantics: **pausing stops picking up
   new uploads, but uploads already being processed run to completion and
   their callbacks are still delivered.**

## Goals

- Tags become a single-level key→value map carried by each document.
- Tags can be passed at upload (sync and async) and are returned in the async
  callback payload.
- Admins can pause/resume the upload queue from the existing
  `/admin/upload_requests` page; callbacks and housekeeping are never blocked
  by a pause.

## Non-goals

- No tag registry / controlled vocabulary (explicitly removed).
- No partial tag patching API (the replace endpoint takes the full map).
- No pause of the callback delivery or reconcile jobs.
- No full-text search, versioning, backups (unchanged from main spec).

---

## Feature 1 — Key→value tags

### Data model

- `documents.tags`: `jsonb NOT NULL DEFAULT '{}'` + **GIN index**.
- `upload_requests.tags`: `jsonb NOT NULL DEFAULT '{}'` — the tags requested
  at upload, applied to the document by `StoreDocumentJob`.
- The `tags` and `document_tags` tables and the `Tag` / `DocumentTag` models
  are **removed**.

**Migration** (single corrective migration, mirrors the repo's style):

1. Add `documents.tags` and `upload_requests.tags` columns + GIN index on
   `documents.tags`.
2. Backfill: each existing flat tag name becomes a key with `null` value
   (`"facture"` → `{"facture": null}`).
3. Drop `document_tags`, then `tags`.

### Validation (shared by Document and UploadRequest)

A single validator (concern or custom validator class) enforces:

- value must be a JSON object (Hash) — single level, no nesting;
- keys: strings, stripped + downcased (continuity with the old
  `normalizes :name` behavior), non-empty after normalization, ≤ 100 chars;
- values: strings (stripped) or `null`, ≤ 500 chars — any other type
  (array, hash, number, bool) is rejected;
- at most 50 entries per document.

Violations surface as model validation errors → API 422 / admin form error.
Normalization happens on assignment so stored jsonb is already canonical.

### API changes

| Endpoint | Change |
| --- | --- |
| `POST /api/v1/documents` (sync + async) | New optional `tags` param, nested map form: `tags[type]=facture`. Async: invalid tags are rejected with 422 **before** the 202 is issued. |
| Async callback payload | Gains `tags`: the created document's map when `completed`, `null` when `failed`. Full shape: `{ request_id, status, document_id, error_message, tags }`. |
| `document_json` (index/show/create responses) | `tags` changes from an array of names to the map. **Breaking**, accepted (internal system). |
| `GET /api/v1/documents` filter | `?tag=type` → documents having the key (`tags ? 'type'`, GIN-served). `?tag=type&tag_value=facture` → containment `tags @> '{"type":"facture"}'` (GIN-served). Filter params get the same normalization as stored tags (key strip+downcase, value strip). `tag_value` without `tag` → 400. |
| `PUT /api/v1/documents/:id/tags` | **New**: replaces the document's whole tag map (map semantics — client owns the full map). Returns `document_json`. Replaces `POST /api/v1/documents/:id/tags`, which is removed. |
| `GET /api/v1/tags`, `POST /api/v1/tags` | **Removed** (no registry). |

Department authorization for the new/changed endpoints follows the existing
pattern (`authorize_department!` on the document's department).

### Admin UI changes

- **Tags section removed**: `Admin::TagsController`, its views, routes, and
  any dashboard link.
- **Document forms** (new + edit): the tag-registry picker is replaced by a
  textarea, one `key=value` pair per line; a line without `=` is a key with
  null value. Parse errors render as form errors.
- **Documents index**: tags displayed as `key=value` (`key` alone when the
  value is null); the tag filter field filters by key.
- **Upload requests index**: new column showing the requested tags.

---

## Feature 2 — Upload queue pause

### Queue topology

- `StoreDocumentJob` moves to a dedicated **`uploads`** queue.
- `DeliverCallbackJob` and `ReconcileUploadRequestsJob` stay on `default`.

Pausing `uploads` therefore never blocks callback delivery or housekeeping.
Solid Queue's native pause stops *claiming* new jobs; an already-running job
finishes normally and its callback (on `default`) is delivered — exactly the
required semantics.

### Admin UI

On `/admin/upload_requests`, above the filter/table:

- a banner showing queue state (**active** / **paused**) and live counts of
  `pending` / `processing` upload requests;
- a Pause / Resume button. Routes:
  `resource :upload_queue_pause, only: [:create, :destroy]` under `admin`
  (create = pause, destroy = resume), backed by
  `SolidQueue::Queue.find_by_name("uploads").pause` / `.resume`.

### Reconcile interaction

`ReconcileUploadRequestsJob#recover_stuck_pending_requests` **skips
re-enqueueing while the `uploads` queue is paused** — otherwise every run
after `STUCK_AFTER` would flood the paused queue with duplicate jobs
(harmless thanks to the `claim` guard, but noisy). Processing-recovery and
purges keep running.

### Environments

- **Production**: unchanged topology (Solid Queue in Puma).
- **Development**: switches to Solid Queue in Puma, with a
  `ged_development_queue` database mirroring production
  (`config.solid_queue.connects_to` moves out of production-only config), so
  the pause is testable locally.
- **Test**: keeps the `:test` ActiveJob adapter for job assertions; the pause
  controller tests exercise `SolidQueue::Queue` against the queue schema
  loaded in the test setup (queue database entry added for test, prepared by
  `db:test:prepare`).

---

## Error contract (unchanged pattern)

- Malformed `tags` (nesting, non-string values, over limits) → 422 with a
  validation message.
- `tag_value` without `tag` → 400.
- Pause/resume actions require admin session (existing Devise guard).

## Testing

- Model: tag map validation matrix (nesting, types, limits, normalization).
- API: sync + async create with tags; callback payload includes the map
  (success) / `null` (failure); key and key+value filters; `PUT .../tags`
  replace; removed endpoints return 404.
- Admin: document form textarea round-trip; upload-requests banner, pause and
  resume actions; tags column.
- Jobs: `StoreDocumentJob` applies `upload_request.tags`; reconcile skips
  pending re-enqueue while paused.

## Documentation

- Update `AGENTS.md`: tags are a jsonb KV map (no registry), `uploads` queue +
  admin pause, API surface changes.
- This spec records the accepted API breaks: `document_json.tags` array→map,
  `?tag=` now matches keys, `/api/v1/tags` and `POST /documents/:id/tags`
  removed.
