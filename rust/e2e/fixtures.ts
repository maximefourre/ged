import { test as base, type Browser } from '@playwright/test';

/**
 * The stock Playwright browser fixture closes the browser after the worker,
 * but the migration spec requires an explicit `finally` (headless Chrome
 * otherwise keeps running). Override only `browser`; context and page stay
 * the runner's, so `storageState` from the config still applies.
 */
export const test = base.extend<{}, { browser: Browser }>({
  browser: [
    async ({ playwright }, use) => {
      const browser = await playwright.chromium.launch({ headless: true });
      try {
        await use(browser);
      } finally {
        await browser.close();
      }
    },
    { scope: 'worker' },
  ],
});
