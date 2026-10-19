import { build, Platform, Arch } from 'electron-builder';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import console from 'node:console';
if (process.platform !== 'darwin') throw new Error('Use windows:installer on a Windows host. This command is the macOS cross-build path.');
process.env.CSC_IDENTITY_AUTO_DISCOVERY = 'false';
execFileSync('python3', ['scripts/setup-windows-cross.py'], { stdio: 'inherit' });
const tools = JSON.parse(readFileSync('.toolchains/windows/environment.json', 'utf8')).directory;
const bin = path.join(tools, 'bin');
const target = 'x86_64-pc-windows-gnullvm';
execFileSync('rustup', ['target', 'add', target], { stdio: 'inherit' });
execFileSync('cargo', ['build', '--release', '--locked', '--manifest-path', 'engine/Cargo.toml', '--target', target], { stdio: 'inherit', env: {
  ...process.env, CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER: path.join(bin, 'x86_64-w64-mingw32-clang'),
  CC_x86_64_pc_windows_gnullvm: path.join(bin, 'x86_64-w64-mingw32-clang'), AR_x86_64_pc_windows_gnullvm: path.join(bin, 'llvm-ar'),
  RUSTFLAGS: '-C target-feature=+crt-static',
} });
const engine = path.resolve(`engine/target/${target}/release/drift-engine.exe`);
const imports = execFileSync(path.join(bin, 'llvm-readobj'), ['--coff-imports', engine], { encoding: 'utf8' });
const names = [...imports.matchAll(/Name: ([^\r\n]+\.dll)/gi)].map(match => match[1]);
const system = /^(api-ms-|ext-ms-)|^(kernel32|ntdll|ws2_32|wsock32|advapi32|userenv|bcrypt|bcryptprimitives|iphlpapi|ole32|shell32|crypt32|msvcrt|ucrtbase|user32|gdi32|secur32|oleaut32|version|normaliz|rpcrt4|psapi|wldp)\.dll$/i;
const extra = [];
for (const name of names.filter(name => !system.test(name))) {
  const candidates = [path.join(tools, 'x86_64-w64-mingw32/bin', name), path.join(bin, name)];
  const file = candidates.find(file => existsSync(file));
  if (!file) throw new Error(`Unresolved Windows runtime dependency: ${name}`);
  extra.push({ from: file, to: name });
}
const electron = JSON.parse(readFileSync('node_modules/electron/package.json', 'utf8')).version;
const outputs = await build({ targets: Platform.WINDOWS.createTarget('nsis', Arch.x64), config: {
  appId: 'local.drift.transfer', productName: 'Drift', electronVersion: electron,
  directories: { app: 'desktop', output: 'release/windows' }, files: ['main.cjs', 'preload.cjs', 'package.json'], asar: true,
  artifactName: '${productName}-${version}-windows-${arch}.${ext}',
  extraResources: [{ from: engine, to: 'drift-engine.exe' }, { from: 'dist/ui', to: 'ui' }, ...extra],
  win: { icon: 'public/icons/phone-512.png', signExecutable: false },
  nsis: { oneClick: false, perMachine: false, allowToChangeInstallationDirectory: true, deleteAppDataOnUninstall: false },
} });
mkdirSync('release/windows', { recursive: true });
const files = [];
for (const file of outputs) {
  if (!statSync(file).isFile()) continue;
  const hash = createHash('sha256'); for await (const block of createReadStream(file)) hash.update(block);
  files.push({ file: path.basename(file), sha256: hash.digest('hex'), bytes: statSync(file).size });
}
writeFileSync('release/windows/BUILD.json', JSON.stringify({ platform: 'windows', architecture: 'x64', crossCompiledOn: process.platform, signed: false, testedOnWindows: false, engineImports: names, files }, null, 2) + '\n');
console.log('Windows installer built by cross-compilation. Windows installation/runtime testing is still required.');
