# Playwright against the admin UI

The suite talks only to `GED_BASE_URL`. It does not start a server. Compose
`web` is `ged serve`, so the URL is that container's published port.

## What must already be up

From the repository root:

```sh
bin/worktree-stack up
bin/worktree-stack seed
```

`bin/worktree-stack urls` prints `web=<port>`. The admin password is
`GED_ADMIN_PASSWORD` from `.env`.

## Suite

```sh
cd rust/e2e
npm ci
npx playwright install chromium
GED_BASE_URL="http://127.0.0.1:${WEB_PORT}" \
  GED_ADMIN_USERNAME=admin \
  GED_ADMIN_PASSWORD="$(the value from .env)" \
  GED_BACKEND=rust \
  npx playwright test
```

`GED_ADMIN_USERNAME` defaults to `admin` when it is unset. `GED_BASE_URL` and
`GED_ADMIN_PASSWORD` are required. `bin/ci` runs this when `GET /up` on the
worktree stack answers, and prints `Playwright: skipped` otherwise.

Chromium is headless and closed in `fixtures.ts` (`finally`). One worker, no
retries. `global-setup` runs `cargo run -q -p parity --bin admin_session`,
which logs in once and prints the session cookie.
