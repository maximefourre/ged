# OpenAPI documentation + Bruno test client

**Date:** 2026-07-11
**Status:** Approved

## Goal

Publish a machine-readable contract for the `/api/v1` HTTP API and ship a
ready-to-run Bruno collection so callers (and QA) can exercise every endpoint
without hand-writing requests. Documentation only — no change to API behaviour.

## Scope

In: an OpenAPI 3.1 spec covering all `/api/v1` endpoints, an HTTP endpoint that
serves the spec, a browsable Redoc page, and a committed Bruno collection.
Out: generated typed SDKs, contract-testing the spec against the app, changes
to any existing endpoint.

## Components

### 1. OpenAPI spec — `docs/api/openapi.yaml`

Hand-written OpenAPI 3.1.0 (rswag is RSpec-based; this repo is Minitest, so a
generator is a poor fit — a hand-authored spec is the source of truth). Covers:

- `GET /api/v1/ping`
- `GET /api/v1/documents` (filters: `department_id`, `filename`, `tag`,
  `tag_value`; pagination: `limit`, `offset`)
- `POST /api/v1/documents` — multipart, sync (default) and async (`callback_url`
  → 202); `department_id`, `file`, `tags[<key>]`
- `GET /api/v1/documents/{id}`
- `GET /api/v1/documents/{id}/download`
- `DELETE /api/v1/documents/{id}`
- `PUT /api/v1/documents/{id}/tags` — replaces the whole tag map
- `GET /api/v1/departments`, `POST /api/v1/departments`
- `GET /api/v1/upload_requests/{id}`

Details:

- `servers`: `http://localhost:3000`.
- `securitySchemes`: `bearerAuth` (`http`/`bearer`, `JWT`) applied globally;
  the description explains that tokens come from the OAuth2
  `client_credentials` flow at `http://localhost:9292/token` (dev provider). An
  `oauth2` `clientCredentials` scheme documents that `tokenUrl` too.
- Schemas: `Document`, `Department`, `UploadRequestStatus`, `AsyncAccepted`,
  `Error`. Response shapes mirror `DocumentJson`, the departments/upload_request
  controllers exactly.
- Error responses per the contract: 400 (missing/invalid param), 401
  (invalid/missing token), 403 (unregistered client / forbidden department),
  404, 422 (validation). `Error` is `{ "error": string }`.

Committed under `docs/` — `.dockerignore` does not exclude `docs/` and the
Dockerfile does `COPY . .`, so the file is present in the production image and
readable at runtime.

### 2. Serving — `ApiDocsController`

`ApiDocsController < ActionController::Base` (no Devise, no bearer auth — the
API's `Api::V1::BaseController` filter does not apply here). Both routes are
deliberately public: the contract is not secret, and the real endpoints stay
protected.

- `GET /api/v1/openapi.yaml` → reads the committed YAML once (memoized) and
  renders it as `application/yaml`.
- `GET /api/docs` → a small Redoc HTML page that loads the Redoc standalone
  bundle from jsdelivr (major-pinned `redoc@2`) and points at
  `/api/v1/openapi.yaml`. Needs internet reachable from the viewer's browser —
  acceptable for an admin/dev tool; noted in the README.

Routes are added outside the authenticated `api/v1` namespace block.

### 3. Bruno collection — `docs/api/bruno/`

- `bruno.json` — collection manifest.
- `environments/local.bru` — `baseUrl` (`http://localhost:3000`), `tokenUrl`
  (`http://localhost:9292/token`), `clientId`, `clientSecret`, `token`,
  `departmentId`, `documentId`, `requestId`.
- `Auth/Get token.bru` — `POST {{tokenUrl}}` form (`grant_type`,
  `client_id`, `client_secret`); post-response script stores
  `access_token` into the `token` env var so every other request can use
  `auth: bearer {{token}}`.
- Folders Ping / Departments (List, Create) / Documents (List, Upload sync,
  Upload async, Get, Download, Replace tags, Delete) / Upload requests (Get).

### 4. Docs

Short "API docs & Bruno client" section in the README; an entry in AGENTS.md's
implementation map for the two served endpoints and the spec/collection paths.

## Testing

Integration test (`test/integration/api_docs_test.rb`):

- `GET /api/docs` → 200, HTML, references the spec URL.
- `GET /api/v1/openapi.yaml` → 200, `application/yaml`, **no** bearer token
  required, body parses as YAML with `openapi: 3.1.x`.
- Every `/api/v1` path the app routes is present in the spec (guards against the
  spec drifting from the routes).

Then `bin/rails test` (full) and `bin/rubocop` must pass. Rebuild and restart
the Docker stack (`docker compose up --build -d`) and smoke-test both endpoints
with curl.

## Non-goals / risks

- The spec is maintained by hand; the routes-coverage test catches *missing*
  paths but not drift in a path's parameters or response shape. Accepted.
- Redoc via CDN means the docs page is blank without internet in the browser;
  the raw spec endpoint and the Bruno collection have no such dependency.
