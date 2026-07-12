# Read/Write Permissions per Department — Design

**Date:** 2026-07-12
**Status:** Approved (plan validated with user)

## Context

App clients get access to departments through `client_department_accesses` —
today a **binary grant**: the presence of a row means full access (list, show,
download, upload, delete, retag) to that department's whole subtree. The user
needs **least-privilege separation**: a client that can **upload but not read**,
or **read but not upload** — e.g. an ingestion service that only pushes
documents and a separate consumer service that only reads them.

This is an architectural change to the permission model. The permission boundary
is unchanged (still the department subtree); we split the *verb* allowed on it.

## Goals

- Each (client, department) grant carries **independent** `can_read` and
  `can_write` booleans.
- READ gates: list documents, show, download, list departments, poll upload
  status. WRITE gates: create document (sync + async upload), delete, replace
  tags, create sub-department.
- WRITE does **not** imply READ and vice-versa. A write-only client uploading a
  document still gets the created document echoed back — that is the *result of
  its write*, not a read.
- Cascade unchanged: a grant on a parent department applies to the whole subtree;
  child grants never escalate upward. Read and write cascade independently.
- Fully backward compatible: existing grants become `can_read = can_write = true`.

## Non-goals

- No per-document ACLs; no verbs beyond read/write (no separate delete/retag
  scope); no admin-role change (admins still bypass client ACLs entirely).
- No change to the department tree model or to the JWT/JWKS auth path.

## Data model

`client_department_accesses` gains two columns:

- `can_read`  — boolean, NOT NULL, DEFAULT `true`
- `can_write` — boolean, NOT NULL, DEFAULT `true`
- CHECK constraint `at_least_one_permission` (`can_read OR can_write`) — a row
  granting neither is meaningless; "no access" is expressed by the **absence** of
  a row.
- The unique index on `[app_client_id, department_id]` stays: still exactly one
  row per couple.

Column names use the `can_` prefix (not `read`/`write`) to avoid method-name
ambiguity and to read naturally (`access.can_write?`).

## Authorization semantics

- `AppClient#can_access?(department, mode)` — `mode` is `:read` or `:write`,
  **required** (no default, so a forgotten mode fails loudly in tests instead of
  silently granting the wrong verb). Returns true iff some department in
  `department.self_and_ancestors` has a grant row for this client with the mode's
  column `true`.
- `AppClient#accessible_department_ids(mode = :read)` — union of the subtrees of
  the departments granted for `mode`; used to scope list endpoints (always read).
- `Api::V1::BaseController#authorize_department!(department, mode:)` — required
  `mode:` kwarg; raises `NotAuthorizedError` → 403 with body
  `"no read access to this department"` / `"no write access to this department"`.

Per-endpoint mode:

| Endpoint | Mode |
|---|---|
| `GET /documents` (list), `GET /documents/:id`, `GET /documents/:id/download` | READ |
| `GET /departments` (list) | READ |
| `GET /upload_requests/:id` | READ |
| `POST /documents` (sync + async upload) | WRITE |
| `DELETE /documents/:id` | WRITE |
| `PUT /documents/:id/tags` | WRITE |
| `POST /departments` | WRITE |

The shared `find_authorized_document` helper takes `mode:` so `show`/`download`
require READ while `destroy` requires WRITE — this is the main correctness hazard
of the change and is covered by an explicit regression test.

The readonly-metadata list filters (`?uploaded_by`, `?upload_mode`, timestamp
ranges) operate on a scope already bounded by `accessible_department_ids(:read)`,
so they add no new authorization surface.

## Admin UI

The app-client form replaces the single "departments" multi-select with a
**per-department Read/Write checkbox grid**. The controller reconciles submitted
checkboxes into `client_department_accesses` rows: a department checked for read
and/or write upserts a row with those flags; a department left fully unchecked has
its row removed. Index/show display R/W badges per department. `check_box` emits a
hidden `0`, so every rendered department reports its state (checked or not).

## Backward compatibility

The migration backfills existing rows to `can_read = can_write = true` via the
column default, so every already-registered client keeps full access. No request
shape changes; no routes added or renamed (the OpenAPI structure test stays green
— only descriptions are updated).

## Bundled security fix — SSRF callback guard

Shipped on the same branch (see `docs/security/audit-2026-07-12.md`, finding #1).
The async `callback_url` is filtered against non-routable address ranges
(loopback, RFC1918 private, link-local incl. cloud metadata `169.254.169.254`,
CGNAT, and IPv4-mapped IPv6 equivalents) by a new `CallbackUrlPolicy` service,
enforced **syntactically** at request time (fast 422 for IP-literal targets) and
by **DNS resolution** at delivery time in `DeliverCallbackJob` (the real
boundary: refuse + log, no retry). http and https both stay allowed; filtering is
on the resolved IP, not the scheme.

## Testing

Minitest, records created **inline** (the repo uses no fixtures). Coverage:
read-only client → 403 on every write verb; write-only client → 403 on every read
verb and success on upload; **destroy requires write** (regression guard for the
shared helper); mixed cascade (parent RW, child R); `at_least_one_permission`
validation; admin reconciliation (rows created/removed from the grid). SSRF:
`CallbackUrlPolicy` unit tests (loopback/private/link-local/IPv4-mapped, DNS→
private via stubbed `Resolv`) + job refuses private targets without POSTing.
