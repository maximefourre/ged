# GED — document management system

Rails app + PostgreSQL + temporary OAuth2 dev provider. Spec:
`docs/superpowers/specs/2026-07-08-ged-design.md`.

## Development

    brew install mise postgresql@17 && brew services start postgresql@17
    mise install                # ruby from .mise.toml
    bundle install
    bin/rails db:setup          # create + migrate + seed (admin/changeme)
    bin/rails test              # main app tests
    (cd oauth-provider && bundle install && bundle exec rake)  # provider tests
    bin/rails server            # http://localhost:3000 → admin UI

## Deployment (Docker Compose)

Create a `.env` file at the repo root (gitignored; read automatically by
compose — required because every `docker compose` command re-evaluates it):

    printf 'SECRET_KEY_BASE=%s\nGED_ADMIN_PASSWORD=changeme\n' "$(bin/rails secret)" > .env

Then:

    docker compose up --build -d
    docker compose exec web bin/rails db:seed     # first boot only: create admin

## Testing a feature branch in isolation

Each git worktree can run its own stack, side by side with `main` and other
worktrees, via `bin/worktree-stack`. It sets a per-worktree
`COMPOSE_PROJECT_NAME` (namespacing containers, network, and data volumes) and
publishes distinct host ports derived from the worktree name — no manual
`.env`/port juggling:

    bin/worktree-stack up        # build + start (prints the chosen URLs/ports)
    bin/worktree-stack seed      # first boot only: create the admin user
    bin/worktree-stack ps        # any docker compose subcommand passes through
    bin/worktree-stack clean     # ménage: down -v + remove locally-built images

Secrets are reused from the primary tree's `.env` if present, otherwise a
throwaway `.env` is generated (`GED_ADMIN_PASSWORD=changeme`). Override ports
with `GED_WEB_PORT` / `GED_OAUTH_PORT` in the rare event two worktrees collide.
Run from the primary tree it's just the normal stack on 3000 / 9292.

## Bootstrap an application client

1. `docker compose exec oauth-provider bin/register-client my-app` → note the secret
2. Admin UI (http://localhost:3000) → Clients → Register `my-app` + department access
3. `curl -X POST http://localhost:9292/token -d grant_type=client_credentials -d client_id=my-app -d client_secret=<secret>`
4. Call the API: `curl http://localhost:3000/api/v1/documents -H "Authorization: Bearer <token>"`

NOTE: containers reach the provider at `http://oauth-provider:9292` (compose DNS);
tokens fetched from the host at `localhost:9292` carry `iss=http://oauth-provider:9292`
(set via OAUTH_ISSUER), so they validate correctly inside the stack.

## API documentation & test client

The API contract is an OpenAPI 3.1 spec at `docs/api/openapi.yaml`, served by the
`web` service (no auth):

- `http://localhost:3000/api/docs` — browsable reference (Redoc; needs internet
  in the browser to load the bundle)
- `http://localhost:3000/api/v1/openapi.yaml` — the raw spec

A ready-to-run [Bruno](https://www.usebruno.com) collection lives in
`docs/api/bruno/`. Open that folder in Bruno, pick the **local** environment, set
`clientId`/`clientSecret` (secret vars), then run **Auth → Get token** first — it
stores the token so every other request is authenticated. Upload requests need
`file` pointed at a real file and `departmentId` set from **List departments**.

## Cutover to the real IdP

Config change only, on the `web` service: set `GED_OIDC_ISSUER` and
`GED_OIDC_JWKS_URL` to the real IdP, `GED_CLIENT_ID_CLAIM` to the claim that
carries the client id, and `GED_OIDC_AUDIENCE` to GED's audience identifier —
**required** with a shared IdP, otherwise any internal app's token grants
access to GED. Then remove the `oauth-provider` service.

## Verification checklist (manual, after deploy)

- [ ] Admin login works; create departments Finance + HR
- [ ] Register client, grant Finance only
- [ ] Upload to Finance → 201 + uuid; upload to HR → 403
- [ ] List/download/delete document; tag a document; filter by tag
- [ ] Async upload with callback_url (use https://webhook.site) → 202 then callback with document_id
- [ ] Restart stack (`docker compose restart`) → documents still present
