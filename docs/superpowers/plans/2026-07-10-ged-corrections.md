# GED Corrections & Improvements Plan

**Date:** 2026-07-10
**Status:** In progress — batches 1 (1.1, 1.4, 1.5, 1.6) and 2 (1.2, 1.3)
done 2026-07-10; P1 complete
**Source audit:** full read of `docs/superpowers/specs/2026-07-08-ged-design.md`,
the implementation plan, all app code, tests (61 main + 6 provider, all green),
plus empirical verification of the P1-1 bug via `rails runner`.

Each task is independently committable. Follow TDD: write the failing test
described, then fix. Run `bin/rails test && bin/rubocop` before each commit.

---

## P1 — Bugs (correctness / production incidents)

### 1.1 ✅ Deleting a department with client access or upload requests → 500

**Verified empirically.** `client_department_accesses.department_id` and
`upload_requests.department_id` both carry FK constraints to `departments`,
but `Department` declares no association for either. `department.destroy`
raises `ActiveRecord::InvalidForeignKey` (uncaught → 500 in the admin UI)
instead of the graceful `restrict_with_error` path used for children/documents.

- Add to `app/models/department.rb`:
  - `has_many :client_department_accesses, dependent: :destroy` (an access
    grant is just a mapping row — safe to cascade), and
  - `has_many :upload_requests, dependent: :restrict_with_error` (an upload
    request is an audit record — block deletion like documents).
- Test: admin deletes a department granted to a client → succeeds; admin
  deletes a department with upload_requests → redirect with alert, no 500.
- Defensively rescue `ActiveRecord::InvalidForeignKey` in
  `Admin::DepartmentsController#destroy` (and consider the same in
  `Api::V1::BaseController` → 409 or 422) so any future FK can't 500.

### 1.2 ✅ `Auth::JwksCache` — no HTTP timeouts, fetch under the global mutex, no failure backoff

Three compounding problems in `app/services/auth/jwks_cache.rb`:

- `Net::HTTP.get_response` uses default timeouts (60s open + 60s read).
- The fetch happens **inside** `@mutex.synchronize`, so while the IdP is slow
  or unreachable every API request in the process queues behind the lock.
- On failure `@fetched_at` is not advanced, so the cache stays "stale" and
  **every** subsequent request re-attempts the fetch (no negative caching).
  Net effect: an IdP outage degrades every API call to a serialized
  multi-second timeout instead of fast stale-key answers.

Fix:
- Explicit short timeouts (e.g. `open_timeout: 2`, `read_timeout: 3`).
- Record the failed attempt time and back off (e.g. don't retry more than
  once per 30s while keys are stale-but-present).
- Keep serving stale keys without taking the fetch path on every request
  (fetch outside the lock or single-flight the refresh).
- Test: with a primed cache and a JWKS endpoint stubbed to timeout/error,
  token validation still succeeds fast and only one refresh attempt is made
  within the backoff window.

### 1.3 ✅ No JWKS re-fetch on unknown `kid` (IdP key rotation → up to 5 min of 401s)

`TokenValidator` passes a static key set to `JWT.decode`. After the IdP
rotates its signing key, all tokens fail with 401 until the 300s TTL expires.
The `jwt` gem supports a JWKS **loader lambda** invoked with
`kid_not_found: true`, letting us force-refresh once when an unknown kid
appears (rate-limit the forced refresh to avoid hammering the IdP with
garbage tokens). This matters for the real-IdP cutover.

- Add `Auth::JwksCache#key_set(force: false)` or a `reload_on_miss` loader.
- Test: sign with a new key, JWKS stub returns the rotated set → validation
  succeeds without waiting for TTL.

### 1.4 ✅ `DeliverCallbackJob` — incomplete network-error rescue, no HTTP timeouts

`rescue Errno::ECONNREFUSED, Errno::ETIMEDOUT, SocketError, Timeout::Error, EOFError`
misses at least `Errno::ECONNRESET`, `Errno::EHOSTUNREACH`,
`Errno::ENETUNREACH`, and `OpenSSL::SSL::SSLError` (https callbacks). Those
bypass `retry_on DeliveryFailed` — the job dies with no retry and without the
"permanently failed" log line the spec requires. Also uses default 60s
timeouts.

- Rescue a broader, explicit set (or `SystemCallError` + `IOError` +
  `OpenSSL::SSL::SSLError` + `Timeout::Error`), wrap into `DeliveryFailed`.
- Set explicit open/read timeouts (e.g. 5s/10s) on the callback POST.
- Test: stub `to_raise(Errno::ECONNRESET)` → expect `DeliveryFailed`.

### 1.5 ✅ Callback URL accepted unvalidated (SSRF + job crash)

`callback_url` is stored and later POSTed verbatim. Two issues:
- A non-HTTP URI (`ftp://…`, garbage) only fails **inside the job**
  (`NoMethodError`), which isn't a `DeliveryFailed` → no retry semantics and
  a confusing failure. The client got its 202 long ago.
- SSRF: the server can be pointed at internal targets (its own admin UI, the
  IdP, cloud metadata endpoints). LAN-only app and provisioned clients lower
  the risk, but validation is cheap.

Fix: validate at request time in `UploadRequest` (or the controller):
`callback_url` must parse as `http`/`https` with a host → otherwise 422.
Test: `callback_url: "notaurl"` → 422; `file://…` → 422.

### 1.6 ✅ Malformed multipart `file` param → 500

`params.require(:file)` (API) and `params.require(:document)[:file]` (admin)
assume an `UploadedFile`; a crafted request sending a string reaches
`.original_filename` → `NoMethodError` → 500. Guard with
`respond_to?(:original_filename)` → 400 (API) / redirect with alert (admin).
Test: `post /api/v1/documents, params: { department_id:, file: "hello" }` → 400.

---

## P2 — Functional gaps vs. spec intent

### 2.1 Async upload status is untrackable (no polling endpoint)

The spec promises a "request-tracking reference", but the `request_id` can't
be used anywhere: if the callback never arrives (caller down, retries
exhausted, job lost) the client has no way to learn the outcome.

- Add `GET /api/v1/upload_requests/:id` → `{ request_id, status, document_id,
  error_message }`, authorized via `authorize_department!` on the request's
  department.
- Test: pending → completed transition visible; forbidden department → 403.

### 2.2 Orphaned `pending` upload requests and staging files are never recovered

If the process dies between 202 and job completion (deploy, crash), the
`UploadRequest` stays `pending` forever, the staging file leaks, no callback
is ever sent. `StoreDocumentJob` also never runs twice (`perform_later` fired
once; Solid Queue keeps failed jobs but a lost enqueue is silent).

- Add a small recurring job (Solid Queue `config/recurring.yml`) that:
  - re-enqueues `StoreDocumentJob` for `pending` requests older than N
    minutes whose staging file still exists,
  - marks `failed` (and enqueues the callback) those whose staging file is
    gone,
  - purges staging files / terminal upload_requests older than a retention
    window (e.g. 30 days).
- Test: job-level tests for each branch.

### 2.3 No `aud` (audience) verification — matters for the real-IdP cutover

Today any token from the issuer is accepted regardless of its intended
audience. With the temporary provider that's fine (it only issues tokens for
GED), but a company-wide IdP issues tokens for *every* internal API with the
same `iss`; any app's token would then grant access to GED as whatever its
`sub`/client-id claim says. Add optional `GED_OIDC_AUDIENCE` env var —
when set, `JWT.decode` verifies `aud`. Document it as a cutover requirement
in AGENTS.md/README.

### 2.4 Async failures invisible to humans

`upload_requests` appears nowhere in the admin UI; failed async uploads are
only in logs. Add a read-only `/admin/upload_requests` list (status,
error_message, created_at) — small ERB index, no actions needed.

### 2.5 Spec drift to record (spec says "API … reused internally by the admin UI's controllers")

The admin controllers duplicate the upload/list logic instead of reusing the
API layer. Working as-is; either extract a shared `DocumentCreator` service
(used by both `Api::V1::DocumentsController#create`, the admin controller and
`StoreDocumentJob`) or amend the spec sentence. Recommend the service
extraction — three copies of "build Document + attach + save" already exist.

---

## P3 — Improvements (non-blocking)

1. **Pagination on `GET /api/v1/documents`** (and tags/departments): unbounded
   result sets. Simple `limit`/`offset` params with a default+max page size,
   documented in the JSON contract.
2. **Streaming downloads:** `send_data document.file.download` loads the whole
   blob in memory (API + admin). Async upload exists *because* files can be
   big — downloads have the same problem. Use `document.file.blob.open` +
   `send_file`, or Active Storage proxy routes.
3. **Admin UX:** rename/edit departments (only new/destroy exist); manage a
   document's tags from the admin UI (currently API-only).
4. **Fail fast on missing OIDC config:** `GED_OIDC_ISSUER`/`GED_OIDC_JWKS_URL`
   are only read at request time (KeyError → 500 per request). Validate at
   boot in an initializer (skip in test).
5. **Login brute-force protection:** Rails 8 `rate_limit` on
   `Devise::SessionsController#create` (or `devise :lockable`).
6. **Seeds password caveat:** `find_or_create_by!` never updates an existing
   admin's password; changing `GED_ADMIN_PASSWORD` after first seed is a
   no-op. Either document it or update the password when the env var changes.
7. **FK for `upload_requests.document_id`** (`on_delete: :nullify`) to avoid
   dangling references after document deletion.
8. **`accessible_department_ids` perf:** N queries per tree level per granted
   department on every list call. Fine at current scale; switch to a
   recursive CTE if department counts grow.
9. **Tag assignment race:** concurrent `document.tags << tag` can raise
   `ActiveRecord::RecordNotUnique` (unique index) → 500. Rescue → treat as
   idempotent success. Also `params.require(:name).strip.downcase` 500s on
   non-string params (same class of fix as 1.6).
10. **Missing test coverage worth adding:** custom `GED_CLIENT_ID_CLAIM`;
    JWKS rotation (after 1.3); admin department deletion with grants (after
    1.1); async failure surfaced via polling endpoint (after 2.1).

---

## Suggested execution order

| Batch | Tasks | Rationale |
|---|---|---|
| 1 | 1.1, 1.4, 1.5, 1.6 | Small, isolated, test-first bug fixes |
| 2 | 1.2, 1.3 | JWKS cache rework (one coherent change + tests) |
| 3 | 2.1, 2.2, 2.4 | Async-upload lifecycle completion |
| 4 | 2.3, 2.5 | Cutover preparation + dedup refactor |
| 5 | P3 picks | As time allows; 1 (pagination) and 2 (streaming) first |
