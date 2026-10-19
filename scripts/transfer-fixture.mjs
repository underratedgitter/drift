import { closeSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync, readSync, realpathSync, statSync, writeFileSync, writeSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import process from 'node:process';
import console from 'node:console';
import { Buffer } from 'node:buffer';

function digest(file) {
  const hash = createHash('sha256');
  const buffer = Buffer.alloc(1024 * 1024);
  const fd = openSync(file, 'r');
  try {
    for (;;) {
      const bytes = readSync(fd, buffer, 0, buffer.length, null);
      if (!bytes) break;
      hash.update(buffer.subarray(0, bytes));
    }
  } finally { closeSync(fd); }
  return hash.digest('hex');
}

export function createFixture(destination) {
  const directory = path.resolve(destination);
  if (existsSync(directory)) throw new Error('Fixture destination already exists; refusing to overwrite it.');
  mkdirSync(path.dirname(directory), { recursive: true });
  mkdirSync(directory);
  const files = path.join(directory, 'files');
  mkdirSync(path.join(files, 'nested'), { recursive: true });
  mkdirSync(path.join(files, 'small'));
  writeFileSync(path.join(files, 'empty.bin'), Buffer.alloc(0));
  writeFileSync(path.join(files, 'space name.txt'), 'Drift transfer fixture. No personal data.\n');
  writeFileSync(path.join(files, 'nested', 'hello-世界.txt'), 'Unicode filenames and UTF-8 contents: 世界.\n');
  for (let i = 0; i < 8; i++) writeFileSync(path.join(files, 'small', `${i}.bin`), Buffer.alloc(1024 + i, i));
  const large = path.join(files, 'nested', 'multichunk.bin');
  const fd = openSync(large, 'wx');
  const block = Buffer.alloc(1024 * 1024, 0x63);
  try {
    let remaining = 17 * 1024 * 1024 + 23;
    while (remaining > 0) remaining -= writeSync(fd, block, 0, Math.min(remaining, block.length));
  } finally { closeSync(fd); }
  const names = ['empty.bin', 'space name.txt', 'nested/hello-世界.txt', 'nested/multichunk.bin', ...Array.from({ length: 8 }, (_, i) => `small/${i}.bin`)];
  const entries = names.map(name => ({ path: name, size: statSync(path.join(files, name)).size, sha256: digest(path.join(files, name)) }));
  const manifest = path.join(directory, 'manifest.json');
  writeFileSync(manifest, JSON.stringify({ version: 1, entries }, null, 2) + '\n');
  return { directory, files, manifest, count: entries.length, bytes: entries.reduce((sum, entry) => sum + entry.size, 0) };
}

export function verifyFixture(manifestFile, receivedDirectory) {
  const manifest = JSON.parse(readFileSync(manifestFile, 'utf8'));
  if (manifest.version !== 1 || !Array.isArray(manifest.entries) || !manifest.entries.length || manifest.entries.length > 10000) throw new Error('Invalid fixture manifest.');
  const seen = new Set();
  for (const entry of manifest.entries) {
    if (typeof entry.path !== 'string' || !entry.path || entry.path.includes('\\') || path.posix.isAbsolute(entry.path) || entry.path.split('/').some(part => ['', '.', '..'].includes(part)) || seen.has(entry.path) || !Number.isSafeInteger(entry.size) || entry.size < 0 || !/^[a-f0-9]{64}$/.test(entry.sha256)) throw new Error('Unsafe or invalid fixture manifest entry.');
    seen.add(entry.path);
  }
  const root = realpathSync(receivedDirectory);
  const results = manifest.entries.map(entry => {
    try {
      const file = path.resolve(root, ...entry.path.split('/'));
      const relative = path.relative(root, realpathSync(file));
      if (!relative || path.isAbsolute(relative) || relative === '..' || relative.startsWith(`..${path.sep}`) || !lstatSync(file).isFile() || lstatSync(file).isSymbolicLink()) throw new Error('File is not contained in the selected received directory.');
      if (statSync(file).size !== entry.size) throw new Error('File size differs.');
      if (digest(file) !== entry.sha256) throw new Error('SHA-256 differs.');
      return { path: entry.path, passed: true };
    } catch (error) { return { path: entry.path, passed: false, error: error.message }; }
  });
  return { passed: results.every(result => result.passed), count: results.length, results };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv[2] === 'create' && process.argv.length <= 4) {
      console.log(JSON.stringify(createFixture(process.argv[3] || path.join('release', 'manual-fixtures', randomUUID())), null, 2));
    } else if (process.argv[2] === 'verify' && process.argv.length === 5) {
      const result = verifyFixture(process.argv[3], process.argv[4]);
      console.log(JSON.stringify(result, null, 2));
      if (!result.passed) process.exitCode = 1;
    } else {
      console.error('Usage: node scripts/transfer-fixture.mjs create [new-directory]\n       node scripts/transfer-fixture.mjs verify <manifest.json> <received-root>');
      process.exitCode = 1;
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
