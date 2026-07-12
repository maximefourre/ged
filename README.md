# GED — document management system

Self-hosted document store for a team/company, organized around departments (folders,
the permission boundary) and per-document key→value tags. Accessed primarily by
internal applications over an API secured with OAuth2/JWT bearer tokens, plus a
minimal admin web UI for humans to manage departments, clients, and documents.

Full specification: `docs/superpowers/specs/2026-07-08-ged-design.md`

## Features

- **Department-based access control** — departments are folders and the permission
  boundary; access to a department covers its entire subtree
- **Per-document tags** — key→value map per document, filterable via `?tag=<key>`
  or `?tag=<key>&tag_value=<value>`
- **System-managed metadata** — readonly `uploaded_at`, `updated_at`, `upload_mode`
  block separate from user tags
- **Sync and async uploads** — same endpoint (`POST /api/v1/documents`); async mode
  accepts a `callback_url` to POST the result to when the upload completes
- **Logical deletion with retention** — documents soft-delete for 30 days before
  hard purge; admin can restore from Trash within the window
- **Two auth paths** — application clients authenticate via OAuth2 client_credentials
  (JWT/JWKS); admin users via Devise username/password
- **Admin UI** — manage departments, register application clients, upload/list/tag
  documents, restore from Trash
- **API documentation** — OpenAPI 3.1 spec served at `GET /api/docs` (Redoc
  browser) and `/api/v1/openapi.yaml` (raw spec); includes a ready-to-run
  [Bruno](https://www.usebruno.com) collection for testing

## Installation (local development)

For quick local development without Docker overhead:

    brew install mise postgresql@17 && brew services start postgresql@17
    mise install                # ruby from .mise.toml
    bundle install
    bin/rails db:setup          # create + migrate + seed (admin/changeme)
    bin/rails test              # main app tests
    (cd oauth-provider && bundle install && bundle exec rake)  # provider tests
    bin/dev                     # http://localhost:3000 → admin UI

## Deployment (Docker)

For production or integrated testing (postgres, oauth provider, and rails in
containers):

Create a `.env` file at the repo root (gitignored; read automatically by compose):

    printf 'SECRET_KEY_BASE=%s\nGED_ADMIN_PASSWORD=changeme\n' "$(bin/rails secret)" > .env

Boot the full stack:

    docker compose up --build -d
    docker compose exec web bin/rails db:seed     # first boot only: create admin

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

### Cutover to production IdP

When migrating from the temporary OAuth provider to a real IdP, update the `web`
service environment variables:

- `GED_OIDC_ISSUER` — the real IdP's issuer URL
- `GED_OIDC_JWKS_URL` — the real IdP's JWKS endpoint
- `GED_CLIENT_ID_CLAIM` — the claim containing the client ID (default: `sub`)
- `GED_OIDC_AUDIENCE` — **required** with a shared IdP to prevent token reuse
  across services

Then remove the `oauth-provider` service from `docker-compose.yml`.
