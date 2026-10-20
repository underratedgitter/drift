import { test, expect, webkit, _electron as electron } from '@playwright/test';
import { mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir, release } from 'node:os';
import { join, resolve } from 'node:path';
import { existsSync } from 'node:fs';
import { packagedDesktopExecutable } from './packaged-path.js';

const packagedTransfers = process.env.DRIFT_TEST_PACKAGED_TRANSFERS === '1';
for (const phoneEngine of ['Chromium', 'WebKit']) test(`${packagedTransfers ? 'packaged' : 'native'} app and ${phoneEngine} phone exchange verified files without staging`, async ({ browser }) => {
  test.skip(phoneEngine === 'WebKit' && process.platform === 'darwin' && release().startsWith('23.') && process.env.DRIFT_TEST_WEBKIT !== '1', 'The frozen WebKit on this macOS 14 host hangs creating a blank page. See scripts/probe-webkit.mjs; opt in with DRIFT_TEST_WEBKIT=1.');
  test.setTimeout(120_000);
  const root = await mkdtemp(join(tmpdir(), 'drift-native-test-'));
  const data = join(root, 'engine-data');
  const environment: Record<string, string> = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
  delete environment.ELECTRON_RUN_AS_NODE;
  Object.assign(environment, { DRIFT_DATA_DIR: data, DRIFT_PHONE_PORT: '0', DRIFT_DISABLE_DISCOVERY: '1', DRIFT_ENGINE_PATH: resolve('engine/target/debug', process.platform === 'win32' ? 'drift-engine.exe' : 'drift-engine') });
  if (packagedTransfers) delete environment.DRIFT_ENGINE_PATH;
  const executable = packagedDesktopExecutable();
  if (packagedTransfers && (!executable || !existsSync(executable))) throw new Error('Build the matching native packaged app before running packaged transfer tests.');
  const app = await electron.launch({
    ...(packagedTransfers && executable ? { executablePath: executable } : {}),
    args: [...(packagedTransfers ? [] : [resolve('desktop')]), `--user-data-dir=${join(root, 'electron-profile')}`],
    env: environment,
  });
  // Test-only TLS override in an isolated harness context. Production never
  // ignores TLS errors or changes browser/OS trust settings automatically.
  let phoneBrowser = browser;
  let phoneContext: Awaited<ReturnType<typeof browser.newContext>> | undefined;
  let controlOrigin = '';
  try {
    expect(await app.evaluate(({ app }) => app.isPackaged)).toBe(packagedTransfers);
    expect(await app.evaluate(() => process.arch)).toBe(process.arch);
    phoneBrowser = phoneEngine === 'WebKit' ? await webkit.launch() : browser;
    phoneContext = await phoneBrowser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, acceptDownloads: true });
    const phone = await phoneContext.newPage();
    const desktop = await app.firstWindow();
    await expect(desktop.getByRole('heading', { name: 'Send & receive.' })).toBeVisible();
    controlOrigin = new URL(desktop.url()).origin;
    expect(await desktop.evaluate(() => typeof (window as unknown as { require?: unknown }).require)).toBe('undefined');
    const errors: string[] = [];
    desktop.on('pageerror', error => errors.push(error.message)); phone.on('pageerror', error => errors.push(error.message));
    const response = desktop.waitForResponse(response => response.url().endsWith('/api/phone'));
    await desktop.getByRole('button', { name: 'Devices', exact: true }).click();
    const info = await (await response).json() as { url: string; origin: string; certificate: string };
    expect(info.url).toMatch(/^https:\/\/[^/]+\/phone#[a-f0-9]{48}$/);
    expect(info.certificate).not.toContain('PRIVATE KEY');
    await phone.goto(info.url);
    await expect(phone.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
    await expect(phone.locator('section').first()).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
    await expect(phone.locator('section').first()).toHaveCSS('border-radius', '0px');
    await phone.getByLabel('This phone’s name').fill('Test phone');
    await phone.getByRole('button', { name: 'Request connection' }).click();
    await desktop.getByRole('button', { name: 'Approve', exact: true }).click();
    await phone.getByRole('button', { name: 'Confirm connection' }).click();
    await expect(phone.getByText(/Connected to .* over HTTPS/)).toBeVisible();
    await desktop.getByRole('button', { name: 'Send & receive', exact: true }).click();

    const phoneFile = join(root, 'from-phone.txt');
    await writeFile(phoneFile, 'A real phone-client upload, without an account.');
    await phone.locator('input[type=file]').setInputFiles(phoneFile);
    await phone.getByRole('button', { name: 'Send / resume', exact: true }).click();
    await desktop.getByRole('button', { name: /Approve 1 file/ }).click();
    await expect(phone.locator('.phone-notice')).toContainText('verified on the Mac', { timeout: 30_000 });
    const received = await readdir(join(data, 'received'));
    expect(await readFile(join(data, 'received', received[0], 'from-phone.txt'), 'utf8')).toBe('A real phone-client upload, without an account.');

    const macFile = join(root, 'from-mac.bin');
    const payload = Buffer.alloc(2 * 1024 * 1024 + 19, 0x54);
    await writeFile(macFile, payload);
    await app.evaluate(({ dialog }, selected) => {
      dialog.showOpenDialog = async () => ({ canceled: false, filePaths: [selected] });
    }, macFile);
    await desktop.getByRole('button', { name: 'Choose files', exact: true }).click();
    await expect(desktop.getByRole('heading', { name: '1 file ready to go' })).toBeVisible();
    // Exercise OS-backed File -> isolated preload -> native import, not a fake
    // renderer-created File containing a forged path. Drop must avoid staging.
    await desktop.locator('input[type=file]').first().setInputFiles(macFile);
    await desktop.locator('input[type=file]').first().evaluate(input => {
      const data = new DataTransfer();
      data.items.add((input as HTMLInputElement).files![0]);
      document.querySelector('.drop-zone')!.dispatchEvent(new DragEvent('drop', { bubbles: true, dataTransfer: data }));
    });
    await expect(desktop.getByText('Files selected directly. No staging copy.')).toBeVisible();
    const forged = await desktop.evaluate(async () => {
      try { await window.drift!.importDroppedFiles([new File(['fake'], 'fake.txt')]); return false; }
      catch { return true; }
    });
    expect(forged).toBe(true);
    await desktop.getByLabel('Send to', { exact: true }).selectOption({ label: 'Test phone' });
    await desktop.getByRole('button', { name: 'Send / resume files' }).click();
    await phone.getByRole('button', { name: 'Receive / resume', exact: true }).click();
    await expect(phone.getByRole('heading', { name: 'Verified and ready to save' })).toBeVisible({ timeout: 30_000 });
    const downloading = phone.waitForEvent('download');
    await phone.getByRole('link', { name: 'Save file', exact: true }).click();
    const download = await downloading;
    const saved = join(root, 'verified-download.bin'); await download.saveAs(saved);
    expect(await readFile(saved)).toEqual(payload);
    expect(await readdir(data)).not.toContain('outbox');
    expect((await phoneContext.request.get(`${info.origin}/api/state`)).status()).toBe(404);
    expect(errors).toEqual([]);
    await desktop.screenshot({ path: `test-results/drift-${packagedTransfers ? 'packaged-transfer' : 'native'}-${phoneEngine}.png` });
    await phone.screenshot({ path: `test-results/drift-phone-${packagedTransfers ? 'packaged-' : ''}${phoneEngine}.png`, fullPage: true });
  } finally {
    await phoneContext?.close(); await app.close();
    if (phoneBrowser !== browser) await phoneBrowser.close();
  }
  await expect.poll(async () => fetch(`${controlOrigin}/api/state`).then(response => response.status).catch(() => 0)).toBe(0);
});

test('packaged desktop app starts its bundled engine and shuts it down', async () => {
  const executable = packagedDesktopExecutable();
  test.skip(!executable || !existsSync(executable), 'Build the matching native desktop app or set DRIFT_TEST_PACKAGED_EXECUTABLE to an installed app.');
  if (!executable) return;
  const root = await mkdtemp(join(tmpdir(), 'drift-packaged-test-'));
  const environment: Record<string, string> = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
  delete environment.ELECTRON_RUN_AS_NODE; delete environment.DRIFT_ENGINE_PATH;
  Object.assign(environment, { DRIFT_DATA_DIR: root, DRIFT_PHONE_PORT: '0', DRIFT_DISABLE_DISCOVERY: '1' });
  const app = await electron.launch({ executablePath: executable, args: [`--user-data-dir=${join(root, 'electron-profile')}`], env: environment });
  let origin = '';
  try {
    expect(await app.evaluate(({ app }) => app.isPackaged)).toBe(true);
    expect(await app.evaluate(() => process.arch)).toBe(process.arch);
    const page = await app.firstWindow();
    await expect(page.getByRole('heading', { name: 'Send & receive.' })).toBeVisible();
    await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
    for (const selector of ['.drop-zone', '.nearby']) {
      await expect(page.locator(selector)).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
      await expect(page.locator(selector)).toHaveCSS('border-radius', '0px');
    }
    await page.screenshot({ path: 'test-results/drift-packaged-black.png' });
    origin = new URL(page.url()).origin;
    await page.getByRole('button', { name: 'Devices', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Your phone, in the picture.' })).toBeVisible();
  } finally { await app.close(); }
  await expect.poll(async () => fetch(`${origin}/api/state`).then(response => response.status).catch(() => 0)).toBe(0);
});
