/**
 * Port of the 22 browser tests in test/system/admin_ui_test.rb.
 *
 * Black box: the browser only speaks HTTP to GED_BASE_URL. Names are unique
 * because this stack is the shared development database, not the empty test
 * DB the Capybara suite resets. The session cookie comes from
 * AdminSession::cached (see global-setup.ts); the three login-form tests
 * sign in themselves and never log out the cached session.
 */
import { readFileSync } from 'node:fs';
import { expect, type Locator, type Page } from '@playwright/test';
import { test } from '../fixtures';
import {
  adminPassword,
  adminUsername,
  armConfirm,
  armDismiss,
  createDepartment,
  deptRow,
  docRow,
  filterDocuments,
  pdfBytes,
  pngBytes,
  signIn,
  unique,
  unzip,
  uploadDocument,
  zipBytes,
  type UploadFile,
} from '../support/admin';

const anonymous = { cookies: [], origins: [] };

function pdfFile(name: string): UploadFile {
  return { name, mimeType: 'application/pdf', buffer: pdfBytes() };
}

/** Document id from the row's Download link, not from `data-preview-url`. */
async function documentId(row: Locator): Promise<string> {
  const href = await row.getByRole('link', { name: 'Download' }).getAttribute('href');
  const id = href?.match(/\/admin\/documents\/([^/]+)\/download\/?$/)?.[1];
  expect(id, `Download href ${JSON.stringify(href)}`).toBeTruthy();
  return id!;
}

function previewPath(id: string): RegExp {
  return new RegExp(`/admin/documents/${id}/preview/?$`);
}

/** Preview URL shape plus the bytes the endpoint actually returns. */
async function expectServedPreview(
  page: Page,
  src: string | null,
  id: string,
  contentType: RegExp,
  bytes: Buffer,
): Promise<void> {
  expect(src).toMatch(previewPath(id));
  const response = await page.request.get(src!);
  expect(response.status()).toBe(200);
  expect(response.headers()['content-type'] ?? '').toMatch(contentType);
  expect(Buffer.from(await response.body())).toEqual(bytes);
}

/** Click, then read the file the browser saved. */
async function downloadBytes(page: Page, click: () => Promise<void>): Promise<Buffer> {
  const pending = page.waitForEvent('download');
  await click();
  const download = await pending;
  const filePath = await download.path();
  expect(filePath, 'browser download path').toBeTruthy();
  return readFileSync(filePath!);
}

async function openRowPreview(page: Page, filename: string): Promise<void> {
  const row = docRow(page, filename);
  await row.locator('td.strong').click();
  await expect(page.locator('.doc-split.is-open')).toBeVisible();
  await expect(page.locator('.doc-preview:not([hidden])')).toBeVisible();
}

test.describe('login form', () => {
  test.use({ storageState: anonymous });

  test('2.1 login with valid credentials lands on the documents list', async ({ page }) => {
    await signIn(page, adminPassword());
    await expect(page).toHaveURL(/\/admin\/documents\/?$/);
    await expect(page.getByRole('heading', { level: 1, name: 'Documents' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
  });

  test('2.2 login with a wrong password is rejected and creates no session', async ({ page }) => {
    await page.goto('/admin/documents');
    await page.getByLabel('Username').fill(adminUsername());
    await page.getByLabel('Password').fill('wrong-password');
    await page.getByRole('button', { name: 'Log in' }).click();

    await expect(page.getByRole('button', { name: 'Log out' })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();
    await page.goto('/admin/documents');
    await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();
  });
});

test.describe('authenticated admin', () => {
  test('2.3 create departments with a hierarchy', async ({ page }) => {
    const finance = unique('Finance');
    const hr = unique('HR');
    const invoices = unique('Invoices');

    await createDepartment(page, finance);
    await createDepartment(page, hr);
    await createDepartment(page, invoices, finance);

    const table = page.locator('table');
    await expect(table.locator('td', { hasText: finance })).toBeVisible();
    await expect(table.locator('td', { hasText: hr })).toBeVisible();
    await expect(table.locator('tr', { hasText: `└─ ${invoices}` })).toBeVisible();
  });

  test('2.4 a duplicate department at the same level is rejected', async ({ page }) => {
    const finance = unique('Finance');
    await createDepartment(page, finance);
    await expect(page.locator('td', { hasText: finance })).toBeVisible();

    await page.goto('/admin/departments/new');
    await page.getByLabel('Name', { exact: true }).fill(finance);
    await page.getByRole('button', { name: 'Create' }).click();
    await expect(page.getByText('has already been taken')).toBeVisible();

    await page.goto('/admin/departments');
    await expect(deptRow(page, finance)).toHaveCount(1);
  });

  test('department tree rows collapse and expand', async ({ page }) => {
    const finance = unique('Finance');
    const invoices = unique('Invoices');
    await createDepartment(page, finance);
    await createDepartment(page, invoices, finance);

    await page.goto('/admin/departments');
    await expect(page.locator('tr', { hasText: invoices })).toBeVisible();

    await deptRow(page, finance).locator('[data-tree-toggle]').click();
    await expect(page.locator('tr', { hasText: invoices })).toBeHidden();

    await deptRow(page, finance).locator('[data-tree-toggle]').click();
    await expect(page.locator('tr', { hasText: invoices })).toBeVisible();
  });

  test('2.5 tag keys are normalized to lowercase in the document tags textarea', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));

    await docRow(page, filename).getByRole('link', { name: 'Tags' }).click();
    await page.locator('textarea[name="document[tags_text]"]').fill('Type=Facture\nUrgent');
    await page.getByRole('button', { name: 'Save tags' }).click();

    await expect(page.getByText('Tags updated.')).toBeVisible();
    const row = docRow(page, filename);
    await expect(row.locator('.tag .k', { hasText: /^type$/ })).toBeVisible();
    await expect(row.locator('.tag .v', { hasText: /^Facture$/ })).toBeVisible();
    await expect(row.locator('.tag.solo .k', { hasText: /^urgent$/ })).toBeVisible();
  });

  test('2.6 upload a document and see it in the list attributed to the admin', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));

    const row = docRow(page, filename);
    await expect(row).toBeVisible();
    await expect(row).toContainText(`admin:${adminUsername()}`);
  });

  test('2.7 download the uploaded document returns the original bytes', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));

    const href = await docRow(page, filename).getByRole('link', { name: 'Download' }).getAttribute('href');
    expect(href).toBeTruthy();
    const response = await page.request.get(href!);
    expect(response.status()).toBe(200);
    expect(Buffer.from(await response.body())).toEqual(pdfBytes());
  });

  test('2.8 filter the document list by filename and by department', async ({ page }) => {
    const finance = unique('Finance');
    const hr = unique('HR');
    const invoice = `${unique('invoice')}.pdf`;
    const handbook = `${unique('handbook')}.pdf`;
    await createDepartment(page, finance);
    await createDepartment(page, hr);
    await uploadDocument(page, finance, pdfFile(invoice));
    await uploadDocument(page, hr, pdfFile(handbook));

    await page.goto('/admin/documents');
    await expect(docRow(page, invoice)).toBeVisible();
    await expect(docRow(page, handbook)).toBeVisible();

    await page.locator('input[name="filename"]').fill(invoice);
    await page.getByRole('button', { name: 'Filter' }).click();
    await expect(docRow(page, invoice)).toBeVisible();
    await expect(docRow(page, handbook)).toHaveCount(0);

    await page.goto('/admin/documents');
    await page.locator('select[name="department_id"]').selectOption({ label: hr });
    await page.getByRole('button', { name: 'Filter' }).click();
    await expect(docRow(page, handbook)).toBeVisible();
    await expect(docRow(page, invoice)).toHaveCount(0);
  });

  test('2.9 delete a document removes it from the list', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));
    await page.goto('/admin/documents');
    await expect(docRow(page, filename)).toBeVisible();

    const confirmed = armConfirm(page, `Delete ${filename}?`);
    await docRow(page, filename).getByRole('button', { name: 'Delete' }).click();
    await confirmed;

    await expect(page.getByText('Document moved to trash.')).toBeVisible();
    await expect(docRow(page, filename)).toHaveCount(0);
  });

  test('2.9b a deleted document can be restored from the trash', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));
    await page.goto('/admin/documents');

    const confirmed = armConfirm(page, `Delete ${filename}?`);
    await docRow(page, filename).getByRole('button', { name: 'Delete' }).click();
    await confirmed;
    await expect(page.getByText('Document moved to trash.')).toBeVisible();
    await expect(docRow(page, filename)).toHaveCount(0);

    await page.getByRole('link', { name: 'Trash' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Trash' })).toBeVisible();
    await expect(page.locator('tr', { hasText: filename })).toBeVisible();

    await page.locator('tr', { hasText: filename }).getByRole('button', { name: 'Restore' }).click();
    await expect(page.getByText('Document restored.')).toBeVisible();

    await page.getByRole('link', { name: 'Documents' }).click();
    await expect(docRow(page, filename)).toBeVisible();
  });

  test('clicking a document row opens the split-pane preview in an iframe', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));

    const row = docRow(page, filename);
    const id = await documentId(row);
    expect(await row.getAttribute('data-preview-url')).toMatch(previewPath(id));
    expect(await row.getAttribute('data-content-type')).toBe('application/pdf');
    await openRowPreview(page, filename);

    const frame = page.locator('.doc-preview iframe.doc-preview-frame');
    await expect(frame).toBeVisible();
    await expectServedPreview(
      page,
      await frame.getAttribute('src'),
      id,
      /^application\/pdf(?:;|$)/i,
      pdfBytes(),
    );
    expect(await frame.getAttribute('sandbox')).toBeNull();
  });

  test('an image row renders an <img> preview, not an iframe', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('logo')}.png`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, { name: filename, mimeType: 'image/png', buffer: pngBytes() });

    const row = docRow(page, filename);
    const id = await documentId(row);
    expect(await row.getAttribute('data-preview-url')).toMatch(previewPath(id));
    expect(await row.getAttribute('data-content-type')).toBe('image/png');
    await openRowPreview(page, filename);

    const img = page.locator('.doc-preview img.doc-preview-img');
    await expect(img).toBeVisible();
    await expectServedPreview(page, await img.getAttribute('src'), id, /^image\/png(?:;|$)/i, pngBytes());
    await expect(page.locator('.doc-preview iframe')).toHaveCount(0);
  });

  test('a non-previewable content type shows the fallback card instead of an iframe', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('archive')}.zip`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, {
      name: filename,
      mimeType: 'application/zip',
      buffer: zipBytes(),
    });

    const row = docRow(page, filename);
    expect(await row.getAttribute('data-content-type')).toBe('application/zip');
    await openRowPreview(page, filename);

    await expect(page.getByText('Aucun aperçu disponible pour ce type')).toBeVisible();
    await expect(page.locator('.doc-preview iframe')).toHaveCount(0);
    // Header and fallback card each render a Download link. Capybara's
    // assert_link passes when at least one is present.
    await expect(page.locator('.doc-preview-empty').getByRole('link', { name: 'Download' })).toBeVisible();
  });

  test('the Close button hides the preview and restores the full-width table', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));
    await openRowPreview(page, filename);

    await page.locator('.doc-preview').getByRole('button', { name: 'Close' }).click();

    await expect(page.locator('.doc-split.is-open')).toHaveCount(0);
    await expect(page.locator('.doc-preview')).toBeHidden();
  });

  test('clicking Download/Tags/Delete in a row does not open the preview', async ({ page }) => {
    const finance = unique('Finance');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(filename));

    const row = docRow(page, filename);
    const saved = await downloadBytes(page, () => row.getByRole('link', { name: 'Download' }).click());
    expect(saved).toEqual(pdfBytes());
    await expect(page).toHaveURL(/\/admin\/documents\/?$/);
    await expect(page.locator('.doc-split.is-open')).toHaveCount(0);
    await expect(row).toBeVisible();

    const dismissed = armDismiss(page, `Delete ${filename}?`);
    await row.getByRole('button', { name: 'Delete' }).click();
    await dismissed;
    await expect(page).toHaveURL(/\/admin\/documents\/?$/);
    await expect(page.locator('.doc-split.is-open')).toHaveCount(0);
    await expect(docRow(page, filename)).toBeVisible();
    await expect(page.getByText('Document moved to trash.')).toHaveCount(0);

    await docRow(page, filename).getByRole('link', { name: 'Tags' }).click();
    await expect(page).toHaveURL(/\/admin\/documents\/[^/]+\/edit/);
    await expect(page.locator('.doc-split.is-open')).toHaveCount(0);
  });

  test('select-all enables the bulk toolbar and Delete selected trashes the selection', async ({ page }) => {
    const finance = unique('Finance');
    const one = `${unique('one')}.pdf`;
    const two = `${unique('two')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(one));
    await uploadDocument(page, finance, pdfFile(two));

    await filterDocuments(page, { department: finance });
    await expect(page.locator('tr[data-doc-row]')).toHaveCount(2);
    await expect(page.getByRole('button', { name: 'Delete selected' })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Download ZIP' })).toBeDisabled();

    await page.locator('#select-all').check();
    await expect(page.getByRole('button', { name: 'Delete selected' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Download ZIP' })).toBeEnabled();

    const confirmed = armConfirm(page, 'Move the selected documents to trash?');
    await page.getByRole('button', { name: 'Delete selected' }).click();
    await confirmed;

    await expect(page.getByText('documents moved to trash.')).toBeVisible();
    await expect(docRow(page, one)).toHaveCount(0);
    await expect(docRow(page, two)).toHaveCount(0);

    await page.getByRole('link', { name: 'Trash' }).click();
    await expect(page.locator('tr', { hasText: one })).toBeVisible();
    await expect(page.locator('tr', { hasText: two })).toBeVisible();
  });

  test('bulk ZIP download contains the selected documents', async ({ page }) => {
    const finance = unique('Finance');
    const one = `${unique('one')}.pdf`;
    const two = `${unique('two')}.pdf`;
    await createDepartment(page, finance);
    await uploadDocument(page, finance, pdfFile(one));
    await uploadDocument(page, finance, pdfFile(two));

    await filterDocuments(page, { department: finance });
    await expect(page.locator('tr[data-doc-row]')).toHaveCount(2);
    await page.locator('#select-all').check();
    const boxes = page.locator('tr[data-doc-row] .row-select');
    await expect(boxes).toHaveCount(2);
    await expect(boxes.nth(0)).toBeChecked();
    await expect(boxes.nth(1)).toBeChecked();
    await expect(page.getByRole('button', { name: 'Download ZIP' })).toBeEnabled();

    const selected = await page.locator('tr[data-doc-row]').evaluateAll((rows) =>
      rows.map((row) => {
        const box = row.querySelector('.row-select') as HTMLInputElement | null;
        const id = box?.value ?? '';
        const filename = row.getAttribute('data-filename') ?? '';
        return { id, entry: `${id}-${filename}`, checked: box?.checked === true };
      }),
    );
    expect(selected.every((row) => row.checked && row.id.length > 0)).toBe(true);

    const entries = unzip(
      await downloadBytes(page, () => page.getByRole('button', { name: 'Download ZIP' }).click()),
    );
    expect(entries.size).toBe(2);
    for (const row of selected) {
      expect(entries.has(row.entry), `expected entry ${row.entry}, got ${[...entries.keys()].join(', ')}`).toBe(
        true,
      );
      expect(entries.get(row.entry)).toEqual(pdfBytes());
    }
  });

  test('2.10 deleting a department with documents offers move or trash, then trashes', async ({ page }) => {
    const finance = unique('Finance');
    const archives = unique('Archives');
    const filename = `${unique('report')}.pdf`;
    await createDepartment(page, finance);
    await createDepartment(page, archives);
    await uploadDocument(page, finance, pdfFile(filename));

    await page.goto('/admin/departments');
    const confirmed = armConfirm(page, `Delete ${finance}?`);
    await deptRow(page, finance).getByRole('button', { name: 'Delete' }).click();
    await confirmed;

    await expect(
      page.getByText('This department holds 1 active and 0 trashed document(s).'),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Move documents and delete department' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Trash department and its documents' })).toBeVisible();
    await expect(page.getByRole('heading', { level: 1, name: `Delete ${finance}` })).toBeVisible();

    const listing = await page.context().newPage();
    try {
      await listing.goto('/admin/departments');
      const live = deptRow(listing, finance);
      await expect(live).toHaveCount(1);
      expect(await live.getAttribute('class') ?? '').not.toContain('trashed');
    } finally {
      await listing.close();
    }

    await page.getByRole('button', { name: 'Trash department and its documents' }).click();

    const trashed = page.locator('tr.trashed', { hasText: finance });
    await expect(trashed).toBeVisible();
    await expect(trashed.getByRole('button', { name: 'Restore' })).toBeVisible();

    await trashed.getByRole('button', { name: 'Restore' }).click();

    await expect(page.locator('tr.trashed', { hasText: finance })).toHaveCount(0);
    await expect(deptRow(page, finance).getByRole('link', { name: 'Rename' })).toBeVisible();

    await page.goto('/admin/documents');
    await page.locator('input[name="filename"]').fill(filename);
    await page.getByRole('button', { name: 'Filter' }).click();
    await expect(docRow(page, filename)).toBeVisible();
  });
});

test.describe('logout', () => {
  test.use({ storageState: anonymous });

  test('2.11 after logout a protected page redirects to login', async ({ page }) => {
    await signIn(page, adminPassword());
    await expect(page.getByRole('heading', { level: 1, name: 'Documents' })).toBeVisible();
    await page.getByRole('button', { name: 'Log out' }).click();

    await page.goto('/admin/documents');
    await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Log out' })).toHaveCount(0);
  });
});

test.describe('status and theme', () => {
  test('the status page shows health checks and key metrics', async ({ page }) => {
    await page.goto('/admin/documents');
    await page.getByRole('link', { name: 'Status' }).click();

    await expect(page.getByRole('heading', { level: 1, name: 'Status' })).toBeVisible();
    await expect(page.locator('.health-row')).toHaveCount(4);
    await expect(page.getByText('Database', { exact: true })).toBeVisible();
    await expect(page.getByText('Storage disk', { exact: true })).toBeVisible();
    expect(await page.locator('.stat').count()).toBeGreaterThanOrEqual(5);
  });

  test('the light/dark toggle switches the theme and persists across reloads', async ({ page }) => {
    await page.goto('/admin/documents');
    const initial = await page.evaluate(() => document.documentElement.dataset.theme);
    const toggled = initial === 'dark' ? 'light' : 'dark';

    await page.locator('#theme-toggle').click();
    await expect.poll(() => page.evaluate(() => document.documentElement.dataset.theme)).toBe(toggled);

    await page.goto('/admin/documents');
    await expect.poll(() => page.evaluate(() => document.documentElement.dataset.theme)).toBe(toggled);
  });
});
