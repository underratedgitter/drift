import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { Buffer } from 'node:buffer';
import { createFixture, verifyFixture } from '../scripts/transfer-fixture.mjs';
import { packagedDesktopExecutable } from './ui/packaged-path.ts';

test('packaged executable resolution follows native platform and architecture', () => {
  assert.equal(packagedDesktopExecutable('darwin', 'x64', ''), path.resolve('release/mac/Drift.app/Contents/MacOS/Drift'));
  assert.equal(packagedDesktopExecutable('darwin', 'arm64', ''), path.resolve('release/mac-arm64/Drift.app/Contents/MacOS/Drift'));
  assert.equal(packagedDesktopExecutable('win32', 'x64', ''), path.resolve('release/win-unpacked/Drift.exe'));
  assert.equal(packagedDesktopExecutable('linux', 'x64', ''), null);
  assert.equal(packagedDesktopExecutable('win32', 'arm64', ''), null);
  assert.equal(packagedDesktopExecutable('darwin', 'x64', path.resolve('installed/Drift')), path.resolve('installed/Drift'));
  assert.throws(() => packagedDesktopExecutable('darwin', 'x64', 'relative/Drift'), /absolute path/);
});

test('native CI remains manual, read-only and explicit about runner architectures', () => {
  const workflow = readFileSync('.github/workflows/native-builds.yml', 'utf8');
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /^ {2}(push|pull_request|schedule):/m);
  assert.match(workflow, /contents: read/);
  assert.doesNotMatch(workflow, /contents: write|secrets\./);
  assert.match(workflow, /runner: macos-15-intel, arch: x64/);
  assert.match(workflow, /runner: macos-15, arch: arm64/);
  assert.match(workflow, /runs-on: windows-2025/);
  assert.match(workflow, /DRIFT_TEST_PACKAGED_TRANSFERS: '1'/);
  assert.match(workflow, /DRIFT_TEST_WEBKIT: '1'/);
  assert.match(workflow, /installerInstalled:false/);
  assert.match(workflow, /physicalNetworkTested:false/);
});

test('manual fixture verifies zero-byte, Unicode, multi-window and small files and catches damage', () => {
  mkdirSync('.toolchains', { recursive: true });
  const temp = mkdtempSync(path.resolve('.toolchains/release-tools-test-'));
  try {
    const source = path.join(temp, 'source');
    const fixture = createFixture(source);
    assert.equal(fixture.count, 12);
    const manifest = JSON.parse(readFileSync(fixture.manifest, 'utf8'));
    assert.equal(manifest.entries.find(entry => entry.path === 'empty.bin').size, 0);
    assert.ok(manifest.entries.find(entry => entry.path === 'nested/multichunk.bin').size > 16 * 1024 * 1024);
    assert.throws(() => createFixture(source), /refusing to overwrite/);
    const received = path.join(temp, 'received');
    cpSync(fixture.files, received, { recursive: true });
    assert.equal(verifyFixture(fixture.manifest, received).passed, true);
    writeFileSync(path.join(received, 'small/0.bin'), Buffer.alloc(1024, 0xff));
    assert.equal(verifyFixture(fixture.manifest, received).passed, false);
    rmSync(path.join(received, 'empty.bin'));
    assert.equal(verifyFixture(fixture.manifest, received).results.find(entry => entry.path === 'empty.bin').passed, false);
    const unsafe = path.join(temp, 'unsafe.json');
    writeFileSync(unsafe, JSON.stringify({ version: 1, entries: [{ path: '../outside', size: 0, sha256: '0'.repeat(64) }] }));
    assert.throws(() => verifyFixture(unsafe, received), /Unsafe/);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
