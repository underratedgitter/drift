/* global fetch, document, getComputedStyle, innerWidth */
// This harness controls only its own project-scoped AVD, never a physical device.
import { _android } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, openSync, closeSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import console from 'node:console';
import { setTimeout as wait } from 'node:timers/promises';
import { setTimeout, clearTimeout } from 'node:timers';
import { Buffer } from 'node:buffer';
import { URL } from 'node:url';

const tools = path.resolve('.toolchains');
const env = { ...process.env, ...JSON.parse(readFileSync(path.join(tools, 'environment.json'), 'utf8')),
  ANDROID_AVD_HOME: path.join(tools, 'avd'), ANDROID_USER_HOME: path.join(tools, 'android-test-user') };
const sdk = env.ANDROID_HOME;
const adb = path.join(sdk, 'platform-tools/adb');
const serial = 'emulator-5580';
const name = 'DriftProjectApi35';
const apk = path.resolve('release/android/Drift-android-arm64-x86_64-debug.apk');
if (!existsSync(apk)) throw new Error('Build the emulator APK first: npm run android:package -- --emulator');
mkdirSync(env.ANDROID_AVD_HOME, { recursive: true }); mkdirSync(env.ANDROID_USER_HOME, { recursive: true });
const run = (args, options = {}) => execFileSync(adb, ['-s', serial, ...args], { env, encoding: 'utf8', timeout: 15000, ...options });
try { run(['get-state'], { stdio: ['ignore', 'pipe', 'ignore'] }); throw new Error('Port 5580 already belongs to a device; refusing to modify it.'); } catch (error) { if (error.message.startsWith('Port 5580')) throw error; }
const manager = path.join(sdk, 'cmdline-tools/latest/bin/avdmanager');
execFileSync(manager, ['create', 'avd', '-n', name, '-k', 'system-images;android-35;default;x86_64', '--force'], { env, input: 'no\n', stdio: ['pipe', 'pipe', 'pipe'] });
const log = openSync(path.join(tools, 'emulator-test.log'), 'w');
const emulator = spawn(path.join(sdk, 'emulator/emulator'), ['-avd', name, '-port', '5580', '-no-window', '-no-audio', '-no-boot-anim', '-no-snapshot', '-wipe-data', '-gpu', 'swiftshader_indirect', '-memory', '1536', '-cores', '2', '-camera-back', 'none', '-camera-front', 'none'], { env, stdio: ['ignore', log, log] });
let device, host;
const forwards = [];
try {
  const deadline = Date.now() + 180000;
  while (true) {
    if (emulator.exitCode !== null) throw new Error('Project emulator exited. See .toolchains/emulator-test.log; no physical device was accessed.');
    try { if (run(['shell', 'getprop', 'sys.boot_completed'], { stdio: ['ignore', 'pipe', 'ignore'] }).trim() === '1') break; } catch { /* Wait for our owned emulator, not a tool job. */ }
    if (Date.now() > deadline) throw new Error('Project emulator boot timed out.');
    await wait(1000);
  }
   if (run(['emu', 'avd', 'name']).split(/\r?\n/)[0].trim() !== name) throw new Error('The device on the test port is not the project-owned AVD; refusing installation.');
   const networkDeadline = Date.now() + 30000;
   let interfaceAddresses;
   for (;;) {
     interfaceAddresses = run(['shell', 'ip', '-o', '-4', 'addr', 'show']);
     const address = interfaceAddresses.match(/inet ((?:10\.|192\.168\.|172\.(?:1[6-9]|2\d|3[01])\.)[\d.]+)\//)?.[1];
     // A kernel interface may exist before Android publishes the Wi-Fi network
     // to apps. Wait for ConnectivityService too, not an arbitrary boot delay.
     const connectivity = run(['shell', 'dumpsys', 'connectivity']);
     if (address && connectivity.includes(address) && /Transports: (?:WIFI|ETHERNET)/.test(connectivity)) break;
     if (Date.now() > networkDeadline) throw new Error('Project emulator has no private IPv4 interface before app launch');
     await wait(500);
   }
  run(['install', '-r', apk]);
  run(['shell', 'am', 'start', '-n', 'dev.drift.transfer/dev.drift.app.MainActivity']);
  const devices = await _android.devices();
  device = devices.find(value => value.serial() === serial);
  if (!device) throw new Error('Owned emulator is not visible to the test driver');
  const webview = await device.webView({ pkg: 'dev.drift.transfer' }, { timeout: 45000 });
  const page = await webview.page();
   await page.getByRole('heading', { name: 'Send & receive.' }).waitFor({ timeout: 30000 });
   const theme = await page.evaluate(() => ({
     background: getComputedStyle(document.body).backgroundColor,
     cardFree: ['.drop-zone', '.nearby'].every(selector => {
       const style = getComputedStyle(document.querySelector(selector));
       return style.backgroundColor === 'rgba(0, 0, 0, 0)' && style.borderRadius === '0px';
     }),
     fitsViewport: document.documentElement.scrollWidth <= innerWidth,
   }));
   if (theme.background !== 'rgb(0, 0, 0)' || !theme.cardFree || !theme.fitsViewport) throw new Error('Native Android does not match the black, card-free responsive theme');
  const stateResponse = await page.waitForResponse(response => response.url().endsWith('/api/state'));
  const state = await stateResponse.json();
  if (state.os !== 'android') throw new Error('The native Android engine did not identify its actual platform');
   if (state.invitation.host.startsWith('127.')) {
     throw new Error('Android did not detect its local network interface; refusing to count this as a usable native startup.');
   }
  const peerPort = run(['forward', 'tcp:0', `tcp:${state.invitation.port}`]).trim(); forwards.push(peerPort);
  const hostRoot = mkdtempSync(path.join(tools, 'host-fixture-'));
  host = spawn(path.resolve('engine/target/debug/drift-engine'), ['--name', 'Mac native test', '--ui', path.resolve('dist/ui'), '--data', hostRoot, '--ui-port', '0', '--no-discovery', '--no-phone'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, DRIFT_DESKTOP_PARENT: '' } });
  const privateUrl = await new Promise((resolve, reject) => {
    let text = '';
    const timer = setTimeout(() => reject(new Error('Host fixture startup timed out')), 15000);
    host.stdout.on('data', block => { text += block.toString(); const match = text.match(/http:\/\/127\.0\.0\.1:\d+\/#([a-f0-9]{64})/); if (match) { clearTimeout(timer); resolve(new URL(match[0])); } });
    host.once('exit', () => { clearTimeout(timer); reject(new Error('Host fixture stopped')); });
  });
  const call = async (endpoint, body, method = 'POST') => {
    const response = await fetch(`${privateUrl.origin}/api${endpoint}`, { method, headers: { Authorization: `Bearer ${privateUrl.hash.slice(1)}`, 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
    if (!response.ok) throw new Error(`Host fixture request failed: ${endpoint} (${response.status})`);
    return response.json();
  };
  // ADB forwarding is test-only, for the emulator's isolated NAT. Actual LAN
  // transfers use the discovered/private peer address without port forwarding.
  const paired = await call('/pair', { ...state.invitation, host: '127.0.0.1', port: Number(peerPort) });
  await page.getByRole('button', { name: 'Approve', exact: true }).click();
  await call(`/confirm/${paired.id}`, {});
  const payload = Buffer.alloc(16 * 1024 * 1024 + 23, 0x63);
  const batch = await call('/batch', [{ path: 'native-proof.bin', size: payload.length }]);
  for (let offset = 0; offset < payload.length; offset += 1024 * 1024) {
    const response = await fetch(`${privateUrl.origin}/api/stage/${batch.id}/0?offset=${offset}`, { method: 'PUT', headers: { Authorization: `Bearer ${privateUrl.hash.slice(1)}` }, body: payload.subarray(offset, offset + 1024 * 1024) });
    if (!response.ok) throw new Error(`Fixture staging failed (${response.status}): ${await response.text()}`);
  }
  await call('/send', { connection: paired.id, batch: batch.id });
  await page.getByRole('button', { name: /Approve 1 file/ }).click();
  try {
    await page.getByText('SHA-256 verified').waitFor({ timeout: 30000 });
  } catch (error) {
    const auth = (await stateResponse.request().allHeaders()).authorization;
    const receiver = await page.evaluate(async auth => {
      const response = await fetch('/api/state', { headers: { Authorization: auth } });
      const state = await response.json();
      return state.transfers.map(({status, offsets, error}) => ({status, offsets, error}));
    }, auth);
    const sender = await call('/state', undefined, 'GET');
    console.error('Transfer diagnostics:', JSON.stringify({ receiver, sender: sender.transfers.map(({status, offsets, error}) => ({status, offsets, error})) }));
    throw error;
  }
  const token = (await stateResponse.request().allHeaders()).authorization;
  const nativeState = await page.evaluate(async ({ origin, token }) => {
    const response = await fetch(`${origin}/api/state`, { headers: { Authorization: token } }); return response.json();
  }, { origin: new URL(page.url()).origin, token });
  const received = nativeState.transfers.find(transfer => transfer.direction === 'receive' && transfer.status === 'complete');
  if (!received) throw new Error('Native receiver did not commit a verified transfer');
  const actual = execFileSync(adb, ['-s', serial, 'exec-out', 'run-as', 'dev.drift.transfer', 'cat', `files/engine/received/${received.id}/native-proof.bin`], { env, maxBuffer: payload.length + 65536 });
  if (!actual.equals(payload)) throw new Error('Native Android received bytes differ from the source');
  await page.getByText('Save received files', { exact: true }).click();
  await page.getByRole('button', { name: 'Save native-proof.bin', exact: true }).click();
  const pickerDeadline = Date.now() + 10000;
   while (!run(['shell', 'dumpsys', 'activity', 'activities']).includes('com.android.documentsui')) {
    if (Date.now() > pickerDeadline) throw new Error('Native received-file export did not open the system document picker');
     await wait(250);
   }
   try {
     // Android's Java selector receives the regex source, not JavaScript flags.
     await device.tap({ text: /^[Ss][Aa][Vv][Ee]$/, enabled: true }, { timeout: 15000 });
   } catch (error) {
      const output = process.env.DRIFT_VALIDATION_DIR || 'release/validation-black';
     mkdirSync(output, { recursive: true });
     writeFileSync(`${output}/android-save-picker.png`, execFileSync(adb, ['-s', serial, 'exec-out', 'screencap', '-p'], { env }));
     throw error;
   }
   await page.getByRole('status').filter({ hasText: 'File exported to your selected location.' }).waitFor({ timeout: 30000 });
   const exported = execFileSync(adb, ['-s', serial, 'exec-out', 'cat', '/sdcard/Download/native-proof.bin'], { env, maxBuffer: payload.length + 65536 });
   if (!exported.equals(payload)) throw new Error('Native Android external save-copy differs from the verified source');
   mkdirSync('test-results', { recursive: true });
   // Capture the real WebView viewport; Android's full-page capture can tile
   // the same compositor surface instead of scrolling the native WebView.
   await page.evaluate(() => { document.documentElement.scrollTop = 0; document.body.scrollTop = 0; });
   await page.screenshot({ path: 'test-results/drift-android-black.png', fullPage: false });
   const reportFile = process.env.DRIFT_VALIDATION_DIR ? path.join(process.env.DRIFT_VALIDATION_DIR, 'EMULATOR-TEST.json') : 'release/android/EMULATOR-TEST.json';
   mkdirSync(path.dirname(reportFile), { recursive: true });
   writeFileSync(reportFile, JSON.stringify({ testedAt: new Date().toISOString(), apk: path.basename(apk), apkSha256: createHash('sha256').update(readFileSync(apk)).digest('hex'), emulator: 'AOSP Android 35 x86_64', nativeBoot: true, theme, authenticatedTlsReceive: true, receivedBytesVerified: true, receivedBytes: payload.length, hostFixture: 'current Rust debug build, ADB-forwarded emulator NAT; not a performance measurement', exportPickerOpened: true, externalSaveCopyTested: true, externalSavedBytesVerified: true, physicalDeviceTested: false, physicalLanDiscoveryTested: false }, null, 2) + '\n');
   console.log('PASS: actual Android JNI boot, black card-free UI, account-free TLS pairing/approval, byte-verified transfer, and complete native save-to-Downloads with byte verification. Physical-device testing remains pending.');
} finally {
  if (device) await device.close().catch(() => {});
  if (host) host.kill('SIGINT');
  for (const port of forwards) { try { run(['forward', '--remove', `tcp:${port}`]); } catch { /* Own test forward may already have closed. */ } }
  if (emulator.exitCode === null) emulator.kill('SIGTERM');
  closeSync(log);
}
