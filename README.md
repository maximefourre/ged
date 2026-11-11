# GED — document management system

Self-hosted document store for a team/company, organized around departments (folders,
the permission boundary) and per-document key→value tags. Accessed primarily by
internal applications over an API secured with OAuth2/JWT bearer tokens, plus a
minimal admin web UI for humans to manage departments, clients, and documents.

Architecture diagrams (C4 model, renders on GitHub): [`docs/architecture/c4.md`](docs/architecture/c4.md)
Full specification: `docs/superpowers/specs/2026-07-08-ged-design.md`

## Features

- **Department-based access control** — departments are folders and the permission
  boundary; access to a department covers its entire subtree, with independent
  **read** and **write** grants per client (a client can upload without being able
  to read back, or the reverse)
- **Per-document tags** — key→value map per document, filterable via `?tag=<key>`
  or `?tag=<key>&tag_value=<value>`
- **System-managed metadata** — readonly `uploaded_at`, `updated_at`, `upload_mode`
  block separate from user tags
- **Sync and async uploads** — same endpoint (`POST /api/v1/documents`); async mode
  accepts a `callback_url` to POST the result to when the upload completes
- **Logical deletion with retention** — documents and departments soft-delete
  (30-day retention before hard purge); admin can restore from Trash within the
  window, or move/trash a department's contents before removing it
- **Two auth paths** — application clients authenticate via OAuth2 client_credentials
  (JWT/JWKS); admin users via a username/password session (argon2id)
- **Admin UI** — manage departments, register application clients, upload/list/tag
  documents, restore from Trash
- **API documentation** — OpenAPI 3.1 spec served at `GET /api/docs` (Redoc
  browser) and `/api/v1/openapi.yaml` (raw spec); includes a ready-to-run
  [Bruno](https://www.usebruno.com) collection for testing

## Installation (local development)

The app is Rust (`ged`). **Postgres runs in Docker** — one isolated container
per git worktree, so concurrent worktrees never share a database. There is
**no Homebrew Postgres**. If you still run one, **stop it first** so it
doesn't hold port 5432: `brew services stop postgresql@17`.

    brew install mise
    mise install                # Rust and Ruby from .mise.toml
    bin/worktree-stack db       # Dockerised Postgres for this worktree
    cd rust && cargo test -p ged
    (cd oauth-provider && bundle install && bundle exec rake)

`bin/worktree-stack db-port` prints the host port. The full stack (web,
worker, oauth-provider) is `bin/worktree-stack up`, then
`bin/worktree-stack seed`. Ruby remains only for `oauth-provider/`,
`bin/perf-upload` and `bin/monkey`.

## Deployment (Docker)

`web` (`ged serve`) and `worker` (`ged worker`) share one image and the
`storage` volume. Scale the worker on the same host without rebuilding:

    bin/worktree-stack scale N

Create a `.env` file at the repo root (gitignored; read automatically by compose):

    printf 'SECRET_KEY_BASE=%s\nGED_ADMIN_PASSWORD=changeme\n' "$(openssl rand -hex 64)" > .env

Boot the full stack:

    bin/worktree-stack up
    bin/worktree-stack seed     # first boot: create the admin

## Setup

### Bootstrap an application client

1. Register the client in the OAuth provider:
   ```
   docker compose exec oauth-provider bin/register-client my-app
   ```
   Save the returned secret.

2. Register the client in the admin UI (http://localhost:3000):
   - Navigate to **Clients** → **Register** with client ID `my-app`
   - Grant department access

3. Obtain an OAuth token:
   ```
   curl -X POST http://localhost:9292/token \
     -d grant_type=client_credentials \
     -d client_id=my-app \
     -d client_secret=<secret>
   ```

4. Call the API:
   ```
   curl http://localhost:3000/api/v1/documents \
     -H "Authorization: Bearer <token>"
   ```

   Returns an envelope with documents and pagination metadata:
   ```json
   {
     "documents": [
       {
         "id": "uuid",
         "filename": "invoice.pdf",
         "content_type": "application/pdf",
         "byte_size": 12345,
         "department_id": 1,
         "uploaded_by": "my-app",
         "tags": {"type": "facture"},
         "created_at": "2026-07-16T...",
         "metadata": {
           "uploaded_at": "2026-07-16T...",
           "updated_at": "2026-07-16T...",
           "upload_mode": "sync"
         }
       }
     ],
     "pagination": {
       "limit": 100,
       "offset": 0,
       "total": 1234
     }
   }
   ```

**Note:** Containers reach the provider at `http://oauth-provider:9292` (Docker
Compose DNS); tokens issued by the provider carry `iss=http://oauth-provider:9292`,
which validates correctly inside the stack.

### API documentation & test client

The API spec is served at:
- `http://localhost:3000/api/docs` — interactive reference (Redoc)
- `http://localhost:3000/api/v1/openapi.yaml` — raw OpenAPI 3.1 spec

To test with Bruno:
1. Open the collection in `docs/api/bruno/`
2. Select the **local** environment
3. Set `clientId` and `clientSecret` secret vars
4. Run **Auth → Get token** first — it caches the token for all subsequent requests
5. Upload requests require `file` (path to a real file) and `departmentId` (list from
   **List departments**)

### Observability

Two complementary layers, both optional beyond the built-in status page:

- **Admin status page** — `http://localhost:3000/admin/status` (nav → Status).
  System health checks (database, job workers, OIDC JWKS, storage disk), upload
  pipeline counters (requests by status, queue depths, failed jobs) and
  document/storage stats. Zero dependencies, always available.
- **OpenTelemetry** — the app exports traces (HTTP requests, SQL, jobs,
  outgoing callbacks) and custom metrics (`ged.documents.uploaded`,
  `ged.callbacks.deliveries`, `ged.job.duration`) over OTLP **only when**
  `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Without it, the SDK is never loaded —
  no overhead, nothing to configure.

A local backend (Grafana + Tempo + Prometheus via `grafana/otel-lgtm`) ships as
an optional compose profile with a provisioned **GED overview** dashboard:

    # simplest: let bin/worktree-stack wire everything
    GED_OTEL=1 bin/worktree-stack up

    # or by hand, in .env:
    #   COMPOSE_PROFILES=otel
    #   GED_OTEL_ENDPOINT=http://otel-lgtm:4318

Grafana: `http://localhost:3300` (anonymous admin, dev only). A host-native
`ged serve` exports to the same backend with
`OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318`.

### Cutover to production IdP

When migrating from the temporary OAuth provider to a real IdP, update the `web`
service environment variables:

- `GED_OIDC_ISSUER` — the real IdP's issuer URL
- `GED_OIDC_JWKS_URL` — the real IdP's JWKS endpoint
- `GED_CLIENT_ID_CLAIM` — the claim containing the client ID (default: `sub`)
- `GED_OIDC_TOKEN_BINDING` — how GED isolates itself from other services on a
  shared IdP; `audience` (default) or `scope`
  - `audience`: set `GED_OIDC_AUDIENCE` — **required** (fail-closed), `aud` is
    verified and the app refuses to boot without it. Use this when the IdP
    issues a separate audience per service.
  - `scope`: set `GED_OIDC_REQUIRED_SCOPE` — **required** (fail-closed), every
    token's `scope`/`scp` claim must contain it. Use this when the IdP issues
    one shared audience across every internal service, so `aud` alone can't
    distinguish them.

Then remove the `oauth-provider` service from `docker-compose.yml`.
