// Environment diagnostic only: no Drift server, credentials, files or app code.
import { webkit } from '@playwright/test';
import process from 'node:process';
import console from 'node:console';
import { setTimeout, clearTimeout } from 'node:timers';
const watchdog = setTimeout(() => {
  console.error('WebKit cannot create a blank page within 15 seconds. This host cannot validate Safari compatibility.');
  process.exit(2);
}, 15000);
let browser;
try {
  browser = await webkit.launch({ timeout: 10000 });
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true });
  const page = await context.newPage();
  await page.setContent('<title>Drift environment probe</title>');
  if (await page.title() !== 'Drift environment probe') throw new Error('WebKit blank-page assertion failed');
  console.log('WebKit environment probe passed. Run the opt-in phone E2E next.');
} finally { if (browser) await browser.close(); clearTimeout(watchdog); }
