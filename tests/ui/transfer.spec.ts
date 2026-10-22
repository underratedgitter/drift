import { test, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

// Each test starts real engines (the same engine the Mac app embeds) and drives
// the same interface in a browser, over the loopback control API.
type Running = { child: ChildProcess; url: string; root: string; inbox: string };
const processes: ChildProcess[] = [];

async function start(name: string): Promise<Running> {
  const root = await mkdtemp(join(tmpdir(), 'drift-ui-test-'));
  const inbox = join(root, 'Downloads', 'Drift');
  const binary = resolve('target/debug/drift-engine');
  const child = spawn(binary, ['--name', name, '--host', '127.0.0.1', '--ui-port', '0', '--ui', resolve('dist/ui'), '--data', join(root, 'data'), '--receive', inbox, '--no-discovery']);
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
      if (url) { clearTimeout(timeout); resolve({ child, url: url[0], root, inbox }); }
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

/** Opens the receiver's code dialog and enters that code on the sender. */
async function requestPairing(receivePage: Page, sendPage: Page, wrongCode = false) {
  await receivePage.getByRole('button', { name: 'Pair a device' }).click();
  const address = await receivePage.locator('.pair-invitation p').innerText();
  const code = await receivePage.locator('.pair-invitation code').innerText();
  await receivePage.getByRole('button', { name: 'Close pairing' }).click();
  await sendPage.getByRole('button', { name: 'Pair a device' }).click();
  await sendPage.getByLabel('Local IP address').fill(address.split(':')[0]);
  await sendPage.getByLabel('Port', { exact: true }).fill(address.split(':')[1]);
  await sendPage.getByLabel('Pairing code', { exact: true }).fill(wrongCode ? '0'.repeat(30) : code);
  await sendPage.getByRole('button', { name: 'Request connection' }).click();
}

test('two Macs pair once and send both ways into a visible folder', async ({ browser }) => {
  const studio = await start('Studio Mac');
  const laptop = await start('MacBook Air');
  const studioPage = await browser.newPage();
  const laptopPage = await browser.newPage();
  const browserErrors: string[] = [];
  for (const page of [studioPage, laptopPage]) page.on('pageerror', e => browserErrors.push(e.message));
  await studioPage.goto(studio.url);
  await laptopPage.goto(laptop.url);
  await expect(laptopPage.getByRole('heading', { name: 'Send & receive.' })).toBeVisible();

  await requestPairing(studioPage, laptopPage);
  await expect(studioPage.getByText('MacBook Air wants to connect')).toBeVisible();
  await expect(studioPage.getByLabel('This is one of my devices: accept its files without asking')).toBeChecked();
  await studioPage.getByRole('button', { name: 'Approve', exact: true }).click();
  // One approval: the laptop needs no confirmation step and lists the studio as ready.
  await expect(laptopPage.locator('.nearby .device-row', { hasText: 'Studio Mac' })).toBeVisible();
  await expect(studioPage.locator('.nearby .device-row', { hasText: 'MacBook Air' })).toBeVisible();

  const source = join(laptop.root, 'source');
  await mkdir(source);
  const payload = Buffer.alloc(3 * 1024 * 1024 + 17, 0x72);
  await writeFile(join(source, 'test.bin'), payload);
  await writeFile(join(source, 'notes.txt'), 'Sent without an account.');
  await laptopPage.locator('input[type=file]').nth(1).setInputFiles(source);
  // The only paired Mac is already chosen; own devices' files need no approval.
  await expect(laptopPage.getByLabel('Send to')).toHaveValue(/[a-f0-9]{64}/);
  await laptopPage.getByRole('button', { name: 'Send files', exact: true }).click();
  await expect(studioPage.getByText('Verified', { exact: true })).toHaveCount(1, { timeout: 30_000 });
  await expect(laptopPage.getByText('Verified', { exact: true })).toHaveCount(1, { timeout: 30_000 });
  expect(await readFile(join(studio.inbox, 'source/test.bin'))).toEqual(payload);
  expect(await readFile(join(studio.inbox, 'source/notes.txt'), 'utf8')).toBe('Sent without an account.');

  // Back the other way over the same pairing.
  await studioPage.locator('input[type=file]').first().setInputFiles({ name: 'reply.txt', mimeType: 'text/plain', buffer: Buffer.from('Got it, thanks.') });
  await studioPage.getByRole('button', { name: 'Send files', exact: true }).click();
  await expect(laptopPage.getByText('Verified', { exact: true })).toHaveCount(2, { timeout: 30_000 });
  expect(await readFile(join(laptop.inbox, 'reply.txt'), 'utf8')).toBe('Got it, thanks.');

  await laptopPage.screenshot({ path: 'test-results/drift-mac.png', fullPage: true });
  await expect(laptopPage.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  for (const selector of ['.drop-zone', '.nearby', '.transfer-card']) {
    for (const element of await laptopPage.locator(selector).all()) {
      await expect(element).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
      await expect(element).toHaveCSS('border-radius', '0px');
    }
  }
  await laptopPage.getByRole('button', { name: 'Devices', exact: true }).click();
  await expect(laptopPage.getByText('Paired devices stay paired after restarts')).toBeVisible();
  await expect(laptopPage.getByRole('button', { name: 'Forget' })).toBeVisible();
  expect(browserErrors).toEqual([]);
  await studioPage.close(); await laptopPage.close();
});

test('private control URL is required and settings support keyboard navigation', async ({ page }) => {
  const instance = await start('Private device');
  await page.goto(instance.url.split('#')[0]);
  await expect(page.getByRole('alert')).toContainText('private interface URL');
  await page.goto(instance.url);
  await expect(page.getByText(/This device ·/)).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Network & privacy' })).toBeVisible();
  await expect(page.getByText(instance.inbox)).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.getByRole('button', { name: 'History', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Transfer history.', exact: true })).toBeVisible();
});

test('black, card-free layout fits narrow windows and pairing remains accessible', async ({ page }) => {
  const instance = await start('Small window');
  await page.setViewportSize({ width: 380, height: 740 });
  await page.goto(instance.url);
  await expect(page.getByRole('heading', { name: 'Drop files here.' })).toBeVisible();
  await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 0)');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: 'Pair a device' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Pair with a code.' })).toBeVisible();
  expect(await page.getByRole('dialog').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).not.toBeVisible();
});

test('wrong codes fail; files from someone else’s Mac wait for approval and can be cancelled', async ({ browser }) => {
  const receiver = await start('Approval required');
  const sender = await start('Friend’s Mac');
  const receivePage = await browser.newPage();
  const sendPage = await browser.newPage();
  await receivePage.goto(receiver.url);
  await sendPage.goto(sender.url);
  await requestPairing(receivePage, sendPage, true);
  await expect(sendPage.getByRole('alert')).toBeVisible();
  await expect(receivePage.getByRole('button', { name: 'Approve', exact: true })).toHaveCount(0);
  await sendPage.getByRole('button', { name: 'Dismiss error' }).click();

  await requestPairing(receivePage, sendPage);
  await receivePage.getByLabel('This is one of my devices: accept its files without asking').uncheck();
  await receivePage.getByRole('button', { name: 'Approve', exact: true }).click();
  await expect(sendPage.locator('.nearby .device-row', { hasText: 'Approval required' })).toBeVisible();
  await sendPage.locator('input[type=file]').first().setInputFiles({ name: 'never-approved.txt', mimeType: 'text/plain', buffer: Buffer.from('Do not publish without approval.') });
  await sendPage.getByRole('button', { name: 'Send files', exact: true }).click();
  await expect(receivePage.getByRole('button', { name: /Approve 1 file/ })).toBeVisible();
  await expect(receivePage.getByText('Verified', { exact: true })).toHaveCount(0);
  await sendPage.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(sendPage.locator('.history-status')).toHaveText('cancelled');
  await expect(receivePage.locator('.history-status')).toHaveText('cancelled');
  const published = await readdir(receiver.inbox).catch((error: NodeJS.ErrnoException) => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  expect(published).toEqual([]);
  await sendPage.getByRole('button', { name: 'History', exact: true }).click();
  await sendPage.getByRole('button', { name: 'Clear completed history' }).click();
  await expect(sendPage.getByText('No transfers yet.', { exact: true })).toBeVisible();
  await receivePage.close(); await sendPage.close();
});
