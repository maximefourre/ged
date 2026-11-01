import { expect, type Locator, type Page } from '@playwright/test';
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { crc32, inflateRawSync } from 'node:zlib';

/** Upload bytes owned by this suite. Do not point at Rails `test/fixtures` (removed in T16). */
export const PDF_PATH = path.resolve(__dirname, '../fixtures/report.pdf');

/** Prefix plus 8 hex chars, so parallel leftovers on a shared stack cannot collide. */
export function unique(prefix: string): string {
  return `${prefix}-${randomBytes(4).toString('hex')}`;
}

export function adminUsername(): string {
  const user = process.env.GED_ADMIN_USERNAME;
  if (!user) throw new Error('GED_ADMIN_USERNAME is required');
  return user;
}

export function adminPassword(): string {
  const pass = process.env.GED_ADMIN_PASSWORD;
  if (!pass) throw new Error('GED_ADMIN_PASSWORD is required');
  return pass;
}

export function pdfBytes(): Buffer {
  return readFileSync(PDF_PATH);
}

/** 1×1 PNG. The admin stores the multipart Content-Type on the document row. */
export function pngBytes(): Buffer {
  return Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
    'base64',
  );
}

/** A real stored ZIP so a content sniffer and a client-supplied type agree. */
export function zipBytes(): Buffer {
  return storedZip('hello.txt', Buffer.from('hello'));
}

export type UploadFile = { name: string; mimeType: string; buffer: Buffer };

export function docRow(page: Page, filename: string): Locator {
  return page.locator('tr[data-doc-row]', { hasText: filename });
}

export function deptRow(page: Page, name: string): Locator {
  return page.locator('tr[data-dept-id]', { hasText: name });
}

/** Accept the next `window.confirm`. Register this before the click. */
export function armConfirm(page: Page, message: string): Promise<void> {
  return armDialog(page, message, (dialog) => dialog.accept());
}

/** Refuse the next `window.confirm`. Register this before the click. */
export function armDismiss(page: Page, message: string): Promise<void> {
  return armDialog(page, message, (dialog) => dialog.dismiss());
}

function armDialog(
  page: Page,
  message: string,
  settle: (dialog: { accept: () => Promise<void>; dismiss: () => Promise<void> }) => Promise<void>,
): Promise<void> {
  return new Promise((resolve, reject) => {
    page.once('dialog', (dialog) => {
      const seen = dialog.message();
      settle(dialog)
        .then(() => {
          if (seen !== message) {
            reject(new Error(`confirm was ${JSON.stringify(seen)}, expected ${JSON.stringify(message)}`));
          } else {
            resolve();
          }
        })
        .catch(reject);
    });
  });
}

export async function signIn(page: Page, password: string): Promise<void> {
  await page.goto('/admin/documents');
  await page.getByLabel('Username').fill(adminUsername());
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Log in' }).click();
}

export async function createDepartment(page: Page, name: string, parent?: string): Promise<void> {
  await page.goto('/admin/departments/new');
  await page.getByLabel('Name', { exact: true }).fill(name);
  if (parent) {
    await page.getByLabel('Parent (blank = top-level)').selectOption({ label: parent });
  }
  await page.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByText('Department created.')).toBeVisible();
}

export async function uploadDocument(page: Page, department: string, file: UploadFile): Promise<void> {
  await page.goto('/admin/documents/new');
  await page.getByLabel('Department').selectOption({ label: department });
  await page.locator('[data-dropzone] input[type="file"]').setInputFiles(file);
  await expect(page.locator('[data-dropzone-file]')).toHaveText(file.name);
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(page.getByText('Document uploaded.')).toBeVisible();
}

export async function filterDocuments(
  page: Page,
  opts: { filename?: string; department?: string },
): Promise<void> {
  await page.goto('/admin/documents');
  if (opts.filename !== undefined) {
    await page.locator('input[name="filename"]').fill(opts.filename);
  }
  if (opts.department !== undefined) {
    await page.locator('select[name="department_id"]').selectOption({ label: opts.department });
  }
  await page.getByRole('button', { name: 'Filter' }).click();
}

/** Read a ZIP produced by the admin bulk download (central directory). */
export function unzip(buf: Buffer): Map<string, Buffer> {
  const eocd = findEocd(buf);
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const entries = new Map<string, Buffer>();
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) {
      throw new Error(`bad zip central header at ${p}`);
    }
    const method = buf.readUInt16LE(p + 10);
    const compSize = buf.readUInt32LE(p + 20);
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    const localOff = buf.readUInt32LE(p + 42);
    if (compSize === 0xffffffff || localOff === 0xffffffff) {
      throw new Error('zip64 is not expected for an admin bulk download');
    }
    const name = buf.subarray(p + 46, p + 46 + nameLen).toString('utf8');
    if (buf.readUInt32LE(localOff) !== 0x04034b50) {
      throw new Error(`bad zip local header for ${name}`);
    }
    const localNameLen = buf.readUInt16LE(localOff + 26);
    const localExtraLen = buf.readUInt16LE(localOff + 28);
    const dataOff = localOff + 30 + localNameLen + localExtraLen;
    const compressed = buf.subarray(dataOff, dataOff + compSize);
    if (method === 0) entries.set(name, Buffer.from(compressed));
    else if (method === 8) entries.set(name, inflateRawSync(compressed));
    else throw new Error(`unsupported zip method ${method} for ${name}`);
    p += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

function findEocd(buf: Buffer): number {
  const min = Math.max(0, buf.length - 22 - 65535);
  for (let i = buf.length - 22; i >= min; i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) return i;
  }
  throw new Error('zip end-of-central-directory not found');
}

function storedZip(name: string, data: Buffer): Buffer {
  const nameBuf = Buffer.from(name);
  const crc = crc32(data);
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(0, 8);
  local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(data.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(nameBuf.length, 26);
  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(0, 10);
  central.writeUInt32LE(crc, 16);
  central.writeUInt32LE(data.length, 20);
  central.writeUInt32LE(data.length, 24);
  central.writeUInt16LE(nameBuf.length, 28);
  const cdOff = 30 + nameBuf.length + data.length;
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(46 + nameBuf.length, 12);
  eocd.writeUInt32LE(cdOff, 16);
  return Buffer.concat([local, nameBuf, data, central, nameBuf, eocd]);
}
