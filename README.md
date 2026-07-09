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

    SECRET_KEY_BASE=$(bin/rails secret) GED_ADMIN_PASSWORD=<pw> docker compose up --build -d
    docker compose exec web bin/rails db:seed     # first boot only: create admin

## Bootstrap an application client

1. `docker compose exec oauth-provider bin/register-client my-app` → note the secret
2. Admin UI (http://localhost:3000) → Clients → Register `my-app` + department access
3. `curl -X POST http://localhost:9292/token -d grant_type=client_credentials -d client_id=my-app -d client_secret=<secret>`
4. Call the API: `curl http://localhost:3000/api/v1/documents -H "Authorization: Bearer <token>"`

NOTE: containers reach the provider at `http://oauth-provider:9292` (compose DNS);
tokens fetched from the host at `localhost:9292` carry `iss=http://oauth-provider:9292`
(set via OAUTH_ISSUER), so they validate correctly inside the stack.

## Verification checklist (manual, after deploy)

- [ ] Admin login works; create departments Finance + HR
- [ ] Register client, grant Finance only
- [ ] Upload to Finance → 201 + uuid; upload to HR → 403
- [ ] List/download/delete document; tag a document; filter by tag
- [ ] Async upload with callback_url (use https://webhook.site) → 202 then callback with document_id
- [ ] Restart stack (`docker compose restart`) → documents still present
