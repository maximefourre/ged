import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import path from 'node:path';

/**
 * One admin login for the whole run, via the parity crate's
 * `AdminSession::cached` (Rails allows 10 sign-in attempts per 3 minutes).
 * Login-form tests opt out of this storage state and sign in themselves.
 */
function sessionStillValid(baseURL: string, cookieHeader: string): Promise<boolean> {
  return new Promise((resolve) => {
    const url = new URL('/admin/documents', baseURL);
    const req = http.get(
      url,
      { headers: { Cookie: cookieHeader, Accept: 'text/html' } },
      (res) => {
        res.resume();
        resolve(res.statusCode === 200);
      },
    );
    req.on('error', () => resolve(false));
    req.setTimeout(5000, () => {
      req.destroy();
      resolve(false);
    });
  });
}

export default async function globalSetup(): Promise<void> {
  const baseURL = process.env.GED_BASE_URL;
  if (!baseURL) throw new Error('GED_BASE_URL is required');
  if (!process.env.GED_ADMIN_PASSWORD) throw new Error('GED_ADMIN_PASSWORD is required');

  const authDir = path.join(__dirname, '.auth');
  const authFile = path.join(authDir, 'admin.json');
  if (existsSync(authFile)) {
    const state = JSON.parse(readFileSync(authFile, 'utf8')) as {
      cookies?: { name?: string; value?: string }[];
    };
    const cookie = state.cookies?.[0];
    if (cookie?.name && cookie.value) {
      const ok = await sessionStillValid(baseURL, `${cookie.name}=${cookie.value}`);
      if (ok) return;
    }
  }

  const repoRoot = path.resolve(__dirname, '../..');
  const stdout = execFileSync(
    'cargo',
    ['run', '-q', '-p', 'parity', '--bin', 'admin_session'],
    {
      cwd: path.join(repoRoot, 'rust'),
      env: process.env,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'inherit'],
    },
  );
  const line = stdout
    .split('\n')
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
    .at(-1);
  const eq = line?.indexOf('=') ?? -1;
  if (!line || eq <= 0) {
    throw new Error(`admin-session did not print a cookie, stdout=${JSON.stringify(stdout)}`);
  }
  const name = line.slice(0, eq);
  const value = line.slice(eq + 1);
  const url = new URL(baseURL);
  mkdirSync(authDir, { recursive: true });
  writeFileSync(
    authFile,
    JSON.stringify(
      {
        cookies: [
          {
            name,
            value,
            domain: url.hostname,
            path: '/',
            expires: -1,
            httpOnly: true,
            secure: url.protocol === 'https:',
            sameSite: 'Lax',
          },
        ],
        origins: [],
      },
      null,
      2,
    ),
  );
}
