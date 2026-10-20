import { build, Platform, Arch } from 'electron-builder';
import { execFileSync } from 'node:child_process';
import { createReadStream, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import path from 'node:path';
import process from 'node:process';
const require = createRequire(import.meta.url);
require('electron'); // Ensure the pinned runtime is present before using electronDist.
const target = process.argv[2] ?? 'dir';
const architecture = process.argv[3] ?? process.arch;
if (!['x64', 'arm64'].includes(architecture)) throw new Error('Desktop architecture must be x64 or arm64.');
if (architecture !== process.arch && process.platform !== 'darwin') throw new Error('Cross-architecture packaging is supported only on macOS.');
const permitted = process.platform === 'darwin' ? ['dir', 'dmg', 'zip'] : process.platform === 'win32' ? ['dir', 'nsis', 'zip'] : ['dir', 'AppImage'];
if (!permitted.includes(target)) throw new Error(`Target ${target} cannot be built on ${process.platform}. Use a native build runner.`);
process.env.CSC_IDENTITY_AUTO_DISCOVERY = 'false';
const electron = JSON.parse(readFileSync('node_modules/electron/package.json', 'utf8')).version;
const rustTarget = process.platform === 'darwin' ? (architecture === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin') : null;
const cross = architecture !== process.arch;
if (rustTarget) execFileSync('rustup', ['target', 'add', rustTarget], { stdio: 'inherit' });
execFileSync('cargo', ['build', '--release', '--locked', '--manifest-path', 'engine/Cargo.toml', ...(rustTarget ? ['--target', rustTarget] : [])], { stdio: 'inherit' });
const extension = process.platform === 'win32' ? '.exe' : '';
const engine = `engine/target/${rustTarget ? `${rustTarget}/` : ''}release/drift-engine${extension}`;
if (rustTarget) {
  const actual = execFileSync('lipo', ['-archs', engine], { encoding: 'utf8' }).trim();
  if (actual !== (architecture === 'arm64' ? 'arm64' : 'x86_64')) throw new Error(`Rust engine architecture mismatch: ${actual}`);
}
// Development builds are never published; in CI electron-builder otherwise tries a GitHub release.
const artifacts = await build({ targets: Platform.current().createTarget(target, architecture === 'arm64' ? Arch.arm64 : Arch.x64), publish: 'never', config: {
  appId: 'local.drift.transfer', productName: 'Drift', electronVersion: electron,
  // A host runtime must never be copied into an app for another architecture.
  ...(cross ? {} : { electronDist: 'node_modules/electron/dist' }),
  directories: { app: 'desktop', output: 'release' },
  artifactName: '${productName}-${version}-${os}-${arch}.${ext}',
  files: ['main.cjs', 'preload.cjs', 'package.json'],
  afterPack: async context => {
    if (process.platform !== 'darwin') return;
    const app = path.join(context.appOutDir, 'Drift.app/Contents');
    for (const binary of ['MacOS/Drift', 'Resources/drift-engine', 'Frameworks/Electron Framework.framework/Versions/A/Electron Framework']) {
      const actual = execFileSync('lipo', ['-archs', path.join(app, binary)], { encoding: 'utf8' }).trim().split(/\s+/);
      if (!actual.includes(architecture === 'arm64' ? 'arm64' : 'x86_64')) throw new Error(`Packaged binary has the wrong architecture: ${binary}`);
    }
  },
  extraResources: [{ from: engine, to: `drift-engine${extension}` }, { from: 'dist/ui', to: 'ui' }],
  asar: true, mac: { identity: architecture === 'arm64' ? '-' : null, icon: 'public/icons/phone-512.png', category: 'public.app-category.utilities', extendInfo: { NSLocalNetworkUsageDescription: 'Drift discovers your devices and transfers files directly on your local network.', NSBonjourServices: ['_drift._tcp'] } },
  win: { icon: 'public/icons/phone-512.png', signExecutable: false }, linux: { icon: 'public/icons/phone-512.png', category: 'Utility' },
  nsis: { oneClick: false, allowToChangeInstallationDirectory: true, perMachine: false, deleteAppDataOnUninstall: false },
} });
mkdirSync('release', { recursive: true });
const outputs = [];
for (const file of artifacts) {
  if (!statSync(file).isFile()) continue;
  const hash = createHash('sha256');
  for await (const block of createReadStream(file)) hash.update(block);
  outputs.push({ file: path.relative('release', file), bytes: statSync(file).size, sha256: hash.digest('hex') });
}
const report = JSON.stringify({ builtAt: new Date().toISOString(), platform: process.platform, architecture, buildHostArchitecture: process.arch, target, signed: false, signing: process.platform === 'darwin' && architecture === 'arm64' ? 'ad-hoc (not Developer ID or notarized)' : 'unsigned', installationTest: 'not performed by this packaging command', outputs }, null, 2) + '\n';
writeFileSync(`release/BUILD-${process.platform}-${architecture}.json`, report);
if (!cross) writeFileSync(`release/BUILD-${process.platform}.json`, report);
