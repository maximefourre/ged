# GED (Document Management System) — Design Spec

**Date:** 2026-07-08
**Status:** Approved for planning

## Purpose

A central, self-hosted document store for a team/company of 10+ people, organized
around departments (folders) and cross-cutting tags. The system is accessed mostly
by other internal applications over an API (secured with OAuth2/JWT bearer tokens
issued by an identity provider), with a lightweight admin web UI for occasional
human management of folders, tags, clients, and permissions. Since the company's
real identity provider is not yet accessible, a small local OAuth2 provider is
built as a temporary stand-in (see "Local OAuth2 provider").

## Non-goals (out of scope for this pass)

- Full-text search inside document content (filename/tag/folder search only)
- Document version history
- Automated backups
- Internet exposure / reverse proxy / TLS (runs on a local/office machine for now)
- File type or size restrictions (any file type accepted by default)
- Kafka-topic delivery for async upload results (callback URL only for now)
- Auto-provisioning application clients from JWT claims (manual admin registration only)

## Architecture

A single Ruby on Rails application backed by PostgreSQL, storing files on the local
filesystem via Active Storage, deployed with Docker Compose on a local/office
machine (not exposed to the internet). A separate small OAuth2/OIDC provider
service stands in for the company's real identity provider until access to it is
available (see "Local OAuth2 provider" below).

**Docker Compose services:**
- `web` — main GED Rails app (Puma)
- `postgres` — PostgreSQL database (GED app data)
- `oauth-provider` — small Ruby app issuing dev/test JWTs (see below)

**Volumes:**
- Postgres data volume
- Active Storage local file directory, mounted from the host so documents survive
  container rebuilds
- SQLite file for the `oauth-provider` service

## Local OAuth2 provider (temporary, dev/test stopgap)

The company's real OAuth2/OIDC identity provider is not yet accessible. Until it
is, a small standalone Ruby app fills that role so the full auth flow can be built
and tested end-to-end:

- Small standalone Sinatra app using the `jwt` gem to issue RS256-signed tokens
  (Doorkeeper was considered but rejected: it cannot issue JWT access tokens for
  client_credentials without additional gems and a full Rails app — overkill for
  a throwaway dev stopgap)
- Supports the client_credentials grant and exposes a JWKS endpoint plus OIDC
  discovery, mirroring what the real company IdP is expected to provide
- Uses SQLite for its own client storage (no Postgres dependency); client
  secrets stored as SHA-256 digests (dev-only, not a production credential store)
- Runs as its own `oauth-provider` service in the same Docker Compose stack

**Registering a test application client requires two steps** during this stopgap
period: (1) register the client in `oauth-provider` to obtain a client_id/secret
capable of requesting tokens, and (2) register that same client_id in the GED
admin UI to map it to department access (per the existing manual-provisioning
model below).

**Cutover to the real IdP:** switching to the company's real identity provider
later is a configuration change only — point the GED app's JWKS URL and issuer
config at the real IdP. The `oauth-provider` service can then be disabled/removed.
No changes to the GED app's authorization logic are expected.

## Data model

- **`departments`** — folders; each department is the permission boundary.
  Supports an optional parent/child hierarchy for subfolders within a department;
  access granted to a department applies to all of its subfolders (permissions do
  not vary within a department's own subtree).
- **`tags`** / **`document_tags`** — cross-cutting labels on documents, independent
  of which department folder a document lives in.
- **`documents`** — filename, department (folder), uploader reference, size,
  content type, created_at; file content stored via Active Storage.
- **`app_clients`** — registry of application client identifiers recognized by the
  API, with a human-readable description.
- **`client_department_access`** — join table mapping each `app_client` to the
  department(s)/folder(s) it may access. Access granted is full read/write
  (upload, download, delete, list) within the assigned department(s); there is no
  separate read-only vs. write-only grant in this pass.
- **`admins`** — username + password hash (via Devise) for the admin UI.

## Authentication (two paths)

### 1. Application clients (primary access path)

Applications obtain a JWT from the company's existing external OAuth2/OIDC
identity provider via the client_credentials grant (this token issuance happens
outside this system). They call the GED's API with `Authorization: Bearer
<token>`.

The Rails app validates each incoming token as an OAuth2 resource server:
- Fetches and caches the IdP's JWKS (JSON Web Key Set), refreshed periodically
- Verifies the JWT's signature, expiry, and issuer against the cached JWKS
- Extracts the client identifier from the token
- Looks up the identifier in `app_clients` / `client_department_access` to
  determine which department(s) the request is authorized against

**Provisioning:** application clients are registered manually by an admin via the
admin UI (not auto-provisioned from JWT claims), since the existing IdP's claim
structure is not assumed. The admin creates an `app_client` record and assigns it
to one or more departments.

**JWKS availability:** if the JWKS endpoint is unreachable, the last successfully
fetched key set is used (cached with a refresh retry in the background). If no
key set has ever been successfully fetched, authentication fails closed (all
bearer-token requests are rejected) rather than allowing unverified tokens
through.

### 2. Human admins

Admins log into a minimal server-rendered admin UI (Rails views + Devise) using a
simple built-in username/password, independent of the external OIDC provider.
Session-based login. Used to:
- Create/manage departments (folders) and tags
- Register application clients and assign their department access
- Browse, upload, and manage documents directly

## API surface

Exposed under an API-only namespace (e.g. `/api/v1/...`) for application clients,
and reused internally by the admin UI's controllers:

- Documents: upload, download, delete, list (filterable by department, tag, filename)
- Departments: create, list
- Tags: create, assign to document, list
- Admin-only: register/manage application clients, assign department access

### Document upload: synchronous vs. asynchronous

A single endpoint, `POST /api/v1/documents`, supports two interaction modes,
selected by whether the request includes a `callback_url`:

- **Synchronous (default, no `callback_url`):** the file is stored inline during
  the request. Response is `201 Created` with the document's UUID.
- **Asynchronous (`callback_url` present):** intended for large/slow uploads where
  the caller doesn't want to hold the connection open for storage/persistence to
  finish. The server accepts the uploaded file, responds immediately with `202
  Accepted` and a request-tracking reference, and stores the file via a background
  job (ActiveJob). Once the job completes, the server sends an HTTP `POST` to the
  provided `callback_url` with `{ request_id, document_id }`.
  - Callback delivery is retried with backoff on failure; if retries are
    exhausted, the failure is logged (not silently dropped).
  - A Kafka-topic delivery option (instead of, or alongside, the callback URL) is
    a possible future addition and is explicitly out of scope for this pass — see
    Non-goals.

## Error handling

| Condition | Response |
|---|---|
| Missing/invalid/expired JWT | 401 |
| Valid JWT, but client has no access to the target department | 403 |
| Department/document not found | 404 |
| JWKS endpoint unreachable and no cached keys available | 401 (fail closed) |
| Async upload accepted for background storage | 202 (with request-tracking reference) |
| Async callback delivery fails after retries exhausted | logged, not retried further |

## Testing approach

- **Unit tests:** JWT validation logic (mocked JWKS), department-access
  authorization logic
- **Integration tests:** against a real test PostgreSQL database — full
  upload/list/download flow for an authorized client, a 403 check for a
  client requesting an unauthorized department, and an async upload flow
  verifying the background job stores the file and the callback URL receives
  the document ID
- **Manual verification:** admin UI login, create a department, register an
  application client, call the API with a valid bearer token, confirm access is
  correctly allowed/denied per department

## Deployment

- Docker + Docker Compose on the target local/office machine
- `docker-compose.yml` defines `web`, `postgres`, and `oauth-provider` services
  with persistent volumes
- Initial admin account created via a Rails setup task/seed on first boot
- `web`'s JWKS/issuer config points at `oauth-provider` initially; swapping to the
  company's real IdP later only requires changing this config
