import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

type Running = { child: ChildProcess; url: string; root: string };
const processes: ChildProcess[] = [];

async function start(name: string): Promise<Running> {
  const root = await mkdtemp(join(tmpdir(), 'drift-ui-test-'));
  const binary = resolve('engine/target/debug', process.platform === 'win32' ? 'drift-engine.exe' : 'drift-engine');
  const child = spawn(binary, ['--name', name, '--host', '127.0.0.1', '--ui-port', '0', '--ui', resolve('dist/ui'), '--data', root, '--no-discovery', '--no-phone']);
  processes.push(child);
  return new Promise((resolve, reject) => {
    let output = '';
    const timeout = setTimeout(() => reject(new Error('Drift did not start within 15 seconds')), 15_000);
    child.on('error', reject);
    child.stderr.on('data', chunk => { output += String(chunk); });
    child.on('exit', code => { if (code) reject(new Error(`Engine exited (${code}): ${output}`)); });
    child.stdout.on('data', chunk => {
      output += String(chunk);
      const url = output.match(/http:\/\/127\.0\.0\.1:\d+\/#([a-f0-9]{64})/);
      if (url) { clearTimeout(timeout); resolve({ child, url: url[0], root }); }
    });
  });
}

test.afterEach(async () => {
  for (const child of processes.splice(0)) {
    const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
    child.kill('SIGINT');
    await Promise.race([exited, new Promise(resolve => setTimeout(resolve, 2000))]);
    if (child.exitCode === null) child.kill('SIGKILL');
  }
});

test('two real devices pair, approve and transfer files through the interface', async ({ browser }) => {
  const receiver = await start('Studio Mac');
  const sender = await start('Travel laptop');
  const receivePage = await browser.newPage();
  const sendPage = await browser.newPage();
  const browserErrors: string[] = [];
  for (const page of [receivePage, sendPage]) page.on('pageerror', e => browserErrors.push(e.message));
  await receivePage.goto(receiver.url);
  await sendPage.goto(sender.url);
  await expect(sendPage.getByRole('heading', { name: 'Send & receive.' })).toBeVisible();
  await receivePage.getByRole('button', { name: 'Pair a device' }).click();
  const address = await receivePage.locator('.pair-invitation p').innerText();
  const code = await receivePage.locator('.pair-invitation code').innerText();
  await receivePage.getByRole('button', { name: 'Close pairing' }).click();
  await sendPage.getByRole('button', { name: 'Pair a device' }).click();
  await sendPage.getByLabel('Local IP address').fill(address.split(':')[0]);
  await sendPage.getByLabel('Port', { exact: true }).fill(address.split(':')[1]);
  await sendPage.getByLabel('Pairing code', { exact: true }).fill(code);
  await sendPage.getByRole('button', { name: 'Request connection' }).click();
  await receivePage.getByRole('button', { name: 'Approve', exact: true }).click();
  await sendPage.getByRole('button', { name: 'Devices', exact: true }).click();
  await sendPage.getByRole('button', { name: 'Confirm connection' }).click();
  await expect(sendPage.getByRole('status')).toContainText('Both devices confirmed');
  await sendPage.getByRole('button', { name: 'Send & receive' }).click();
  const source = join(sender.root, 'source');
  await mkdir(source);
  const payload = Buffer.alloc(3 * 1024 * 1024 + 17, 0x72);
  await writeFile(join(source, 'test.bin'), payload);
  await writeFile(join(source, 'notes.txt'), 'Sent without an account.');
  await sendPage.locator('input[type=file]').nth(1).setInputFiles(source);
  await sendPage.getByRole('button', { name: 'Send files', exact: true }).click();
  await receivePage.getByRole('button', { name: /Approve 2 files/ }).click();
  await expect(receivePage.getByText('SHA-256 verified')).toHaveCount(1, { timeout: 30_000 });
  await expect(sendPage.getByText('SHA-256 verified')).toHaveCount(1, { timeout: 30_000 });
  const folders = await readdir(join(receiver.root, 'received'));
  expect(await readFile(join(receiver.root, 'received', folders[0], 'source/test.bin'))).toEqual(payload);
  expect(await readFile(join(receiver.root, 'received', folders[0], 'source/notes.txt'), 'utf8')).toBe('Sent without an account.');
  await sendPage.screenshot({ path: 'test-results/drift-desktop.png', fullPage: true });
  await expect(sendPage.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  for (const selector of ['.drop-zone', '.nearby', '.transfer-card']) {
    for (const element of await sendPage.locator(selector).all()) {
      await expect(element).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
      await expect(element).toHaveCSS('border-radius', '0px');
    }
  }
  await sendPage.setViewportSize({ width: 390, height: 844 });
  expect(await sendPage.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await sendPage.screenshot({ path: 'test-results/drift-mobile-layout.png', fullPage: true });
  expect(browserErrors).toEqual([]);
  await receivePage.close(); await sendPage.close();
});

test('private control URL is required and settings support keyboard navigation', async ({ page }) => {
  const instance = await start('Private device');
  await page.goto(instance.url.split('#')[0]);
  await expect(page.getByRole('alert')).toContainText('private interface URL');
  await page.addInitScript(() => localStorage.setItem('drift-theme', 'light'));
  await page.goto(instance.url);
  await expect(page.getByText(/This device ·/)).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Network & privacy' })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  await expect(page.getByRole('button', { name: /Switch to (light|dark)/ })).toHaveCount(0);
  await page.getByRole('button', { name: 'History', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Transfer history.', exact: true })).toBeVisible();
});

test('black, card-free layout fits narrow screens and pairing remains accessible', async ({ page }) => {
  const instance = await start('Small screen');
  await page.setViewportSize({ width: 320, height: 740 });
  await page.goto(instance.url);
  await expect(page.getByRole('heading', { name: 'Drop files here.' })).toBeVisible();
  await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: 'Pair a device' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  expect(await page.getByRole('dialog').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).not.toBeVisible();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.locator('.panel')).toHaveCSS('border-radius', '0px');
  await expect(page.locator('.panel')).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('phone companion uses the same black, card-free design', async ({ page }) => {
  const instance = await start('Phone preview');
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(new URL('/phone.html', instance.url).href);
  await expect(page.getByRole('heading', { name: 'Send & receive.' })).toBeVisible();
  await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  await expect(page.locator('section')).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  await expect(page.locator('section')).toHaveCSS('border-radius', '0px');
  await expect(page.getByRole('button', { name: 'Request connection' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/drift-phone-black.png', fullPage: true });
});

test('wrong pairing codes fail and pending transfers can be cancelled without publishing files', async ({ browser }) => {
  const receiver = await start('Approval required');
  const sender = await start('Cancel test');
  const receivePage = await browser.newPage();
  const sendPage = await browser.newPage();
  await receivePage.goto(receiver.url);
  await sendPage.goto(sender.url);
  await receivePage.getByRole('button', { name: 'Pair a device' }).click();
  const address = await receivePage.locator('.pair-invitation p').innerText();
  const code = await receivePage.locator('.pair-invitation code').innerText();
  await receivePage.getByRole('button', { name: 'Close pairing' }).click();
  const requestPair = async (value: string) => {
    await sendPage.getByRole('button', { name: 'Pair a device' }).click();
    await sendPage.getByLabel('Local IP address').fill(address.split(':')[0]);
    await sendPage.getByLabel('Port', { exact: true }).fill(address.split(':')[1]);
    await sendPage.getByLabel('Pairing code', { exact: true }).fill(value);
    await sendPage.getByRole('button', { name: 'Request connection' }).click();
  };
  await requestPair('0'.repeat(30));
  await expect(sendPage.getByRole('alert')).toBeVisible();
  await expect(receivePage.getByRole('button', { name: 'Approve', exact: true })).toHaveCount(0);
  await sendPage.getByRole('button', { name: 'Dismiss error' }).click();
  await requestPair(code);
  await receivePage.getByRole('button', { name: 'Approve', exact: true }).click();
  await sendPage.getByRole('button', { name: 'Devices', exact: true }).click();
  await sendPage.getByRole('button', { name: 'Confirm connection' }).click();
  await expect(sendPage.getByRole('status')).toContainText('Both devices confirmed');
  await sendPage.getByRole('button', { name: 'Send & receive', exact: true }).click();
  await sendPage.locator('input[type=file]').first().setInputFiles({ name: 'never-approved.txt', mimeType: 'text/plain', buffer: Buffer.from('Do not publish without approval.') });
  await sendPage.getByRole('button', { name: 'Send files', exact: true }).click();
  await expect(receivePage.getByRole('button', { name: /Approve 1 file/ })).toBeVisible();
  await expect(receivePage.getByText('SHA-256 verified')).toHaveCount(0);
  await sendPage.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(sendPage.locator('.history-status')).toHaveText('cancelled');
  await expect(receivePage.locator('.history-status')).toHaveText('cancelled');
  const published = await readdir(join(receiver.root, 'received')).catch((error: NodeJS.ErrnoException) => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  expect(published).toEqual([]);
  await sendPage.getByRole('button', { name: 'History', exact: true }).click();
  await sendPage.getByRole('button', { name: 'Clear completed history' }).click();
  await expect(sendPage.getByText('No transfers yet.', { exact: true })).toBeVisible();
  await receivePage.close(); await sendPage.close();
});
