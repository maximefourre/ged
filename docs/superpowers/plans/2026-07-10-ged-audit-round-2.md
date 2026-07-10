# GED Audit Round 2 — Findings

**Date:** 2026-07-10 (after completion of the round-1 corrections plan)
**Status:** Complete 2026-07-10. P1 and P2 fixed (commits 994d935, dd408d9);
P3 items 1–6 fixed (commits 600a1c1, 4beba4d, 1acf5d9, d9792aa, 62814bb,
728f030). Only P3-7 (recursive CTE, carried over from round 1's P3-8) remains
deliberately deferred.
**Source audit:** full read of the design spec, all app code, config, Docker
setup, oauth-provider, and tests (99 main + 6 provider, all green; RuboCop and
Brakeman clean). P1-1 verified empirically via `rails runner`.

Overall state: healthy. The round-1 plan
(`2026-07-10-ged-corrections.md`) is fully executed, the implementation is
feature-complete against the spec, and goes beyond it (polling endpoint,
reconciliation job, admin upload-requests view, optional `aud`, login rate
limit, pagination). The findings below are what a fresh pass turns up.

---

## P1 — Bugs

### 1.1 ✅ Tag filter corrupts the `tags` array in the documents list response

**Verified empirically.** `GET /api/v1/documents?tag=X` on a document tagged
`X` and `Y` returns `"tags": ["X"]` — the other tags vanish from the payload.

Cause: in `Api::V1::DocumentsController#index`, `joins(:tags)` +
`where(tags: { name: params[:tag] })` + `includes(:tags)` makes Rails promote
the `includes` to `eager_load` (the WHERE references the included table), so
the tag filter also filters the preloaded `document.tags` collection.

Fix: use `preload(:tags)` instead of `includes(:tags)` (the existing
`joins(:tags)` already satisfies the WHERE), or filter via a subquery on
`document_tags`. Test: document with two tags, filter by one, assert both
names appear in the response `tags` array.

## P2 — Robustness (unhandled 500s on non-scalar params)

Round 1 fixed this class for the `file` param (1.6) and tag `name` (P3-9),
but other API params still 500 on crafted non-string input:

### 2.1 ✅ `limit`/`offset` as arrays or hashes → 500

`paginate` calls `params[:limit].to_i` — `limit[]=2` yields an Array,
`NoMethodError`. Same for `offset`.

### 2.2 ✅ `filename[]=x` on documents#index → 500

`sanitize_sql_like` on an Array raises.

### 2.3 ✅ `department_id[]=…` → 500

`Department.find(<Array>)` returns an Array; `authorize_department!` then
calls `can_access?` on it → `NoMethodError`. Affects documents#index and
documents#create (and departments#create via `parent_id`).

### 2.4 ✅ Non-string `name` on POST /api/v1/departments → garbage row

`params.require(:name)` returns `ActionController::Parameters` for
`name[a]=b`; string-casting stores its `to_s` as the department name (201
with a junk record) instead of 400. Same string check as TagsController.

Fix for all four: a small shared helper in `Api::V1::BaseController`
(e.g. `scalar_param(key)` → 400 unless String/nil), applied to the list
params, plus the string check on departments#create. Tests per endpoint.

### 2.5 ✅ Concurrent duplicate department creation → 500

`Department.create!` has no `ActiveRecord::RecordNotUnique` rescue (the
unique `[parent_id, name]` index wins the race over the validation).
TagsController got this rescue in P3-9; DepartmentsController didn't.
Rescue → 422 like the tags endpoint.

## P3 — Improvements / edge cases (non-blocking)

1. ✅ (600a1c1) **UploadRequest double-processing race:** `ReconcileUploadRequestsJob`
   re-enqueues `StoreDocumentJob` for pending requests older than 10 min
   (`created_at`-based). If the queue backlog ever exceeds 10 min, or the
   process dies between `DocumentCreator.call` and `update!(:completed)`,
   two jobs both see `pending` → duplicate documents + duplicate callbacks.
   Fix: `upload_request.with_lock { return unless pending }` re-check inside
   the job, or an intermediate `processing` status claimed atomically
   (`UploadRequest.where(id:, status: "pending").update_all(status: "processing") == 1`).
2. ✅ (4beba4d) **Orphaned blobs on failed creates:** `DocumentCreator` attaches (which
   uploads the blob immediately) before `save!`; a validation failure leaves
   an unattached blob + file on disk. Purge via
   `ActiveStorage::Blob.unattached.where(created_at: ..1.day.ago).find_each(&:purge_later)`
   in the reconcile job.
3. ✅ (1acf5d9) **Admin UI has no pagination** (documents and upload_requests indexes load
   everything; API got pagination in P3-1). Fine at current volume.
4. ✅ (d9792aa) **Callback payload lacks `error_message`** on failure — callers relying
   only on the callback must re-poll to learn why. Add it for parity with the
   polling endpoint.
5. ✅ (62814bb) **`JwksCache.instance` memoization isn't thread-safe** — two concurrent
   first calls can build two instances (benign: one extra fetch, one instance
   garbage-collected). Wrap in a mutex or use `Concurrent::Lazy` if it ever
   matters.
6. ✅ (728f030) **Thruster is bypassed:** docker-compose maps `3000:3000` straight to
   Puma; Thruster listens on 80 inside the container. Works, but its
   X-Sendfile/compression does nothing. Map `3000:80` (or expose 80) to route
   through Thruster — worthwhile given downloads stream through `send_file`.
7. **P3-8 from round 1** (recursive CTE for `accessible_department_ids`)
   remains deliberately deferred.

## Feature coverage vs spec (no gaps found)

- Documents upload (sync + async w/ callback + polling + reconcile), download
  (streamed), delete, list with department/tag/filename filters + pagination ✓
- Departments create/list (API), full CRUD-minus-reparent (admin) ✓
- Tags create/assign/list (API), manage per document (admin) ✓
- Client registry + department grants, admin-managed ✓
- Two auth paths (JWKS resource server w/ fail-closed cache; Devise admin
  login w/ rate limit) ✓
- Error contract (401/403/404/400/422) ✓ — modulo the P2 500s above
- Docker Compose deploy w/ Solid Queue in Puma, seeds on first boot ✓
