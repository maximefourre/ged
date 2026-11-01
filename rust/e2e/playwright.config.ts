import { defineConfig } from '@playwright/test';

const baseURL = process.env.GED_BASE_URL;
if (!baseURL) {
  throw new Error('GED_BASE_URL is required (this suite never guesses a host)');
}

export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  globalSetup: './global-setup.ts',
  outputDir: './test-results',
  reporter: [['list']],
  use: {
    baseURL,
    headless: true,
    storageState: './.auth/admin.json',
    actionTimeout: 15_000,
    navigationTimeout: 20_000,
  },
});
