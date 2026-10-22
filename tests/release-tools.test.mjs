import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { Buffer } from 'node:buffer';
import { createFixture, verifyFixture } from '../scripts/transfer-fixture.mjs';

test('CI builds only for macOS and publishes only from version tags', () => {
  const workflow = readFileSync('.github/workflows/ci.yml', 'utf8');
  assert.match(workflow, /^permissions:\n {2}contents: read/m);
  for (const runner of workflow.matchAll(/runs-on: (\S+)/g)) assert.match(runner[1], /^(macos-15|ubuntu-24\.04)$/);
  assert.match(workflow, /universal-apple-darwin|app:build/);
  const release = workflow.slice(workflow.indexOf('  release:'));
  assert.match(release, /if: startsWith\(github\.ref, 'refs\/tags\/v'\)/);
  assert.match(release, /contents: write/);
  assert.equal(workflow.split('contents: write').length - 1, 1, 'only the release job may write');
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
