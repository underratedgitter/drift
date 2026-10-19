import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import console from 'node:console';

const root = process.cwd();
const tools = path.join(root, '.toolchains');
const saved = existsSync(path.join(tools, 'environment.json')) ? JSON.parse(readFileSync(path.join(tools, 'environment.json'), 'utf8')) : {};
const environment = { ...process.env, ...saved };
if (!environment.JAVA_HOME || !(environment.ANDROID_HOME || environment.ANDROID_SDK_ROOT)) throw new Error('Install the Android tools first; see scripts/setup-android-tools.py.');
const sdk = environment.ANDROID_HOME || environment.ANDROID_SDK_ROOT;
environment.ANDROID_HOME = sdk; environment.ANDROID_SDK_ROOT = sdk;
const ndk = process.env.ANDROID_NDK_HOME || path.join(sdk, 'ndk', '28.0.13004108');
const host = process.platform === 'darwin' ? 'darwin-x86_64' : 'linux-x86_64';
const bin = path.join(ndk, 'toolchains', 'llvm', 'prebuilt', host, 'bin');
if (!existsSync(bin)) throw new Error('Install NDK 28.0.13004108 or set ANDROID_NDK_HOME. Android builds currently use macOS/Linux hosts.');
execFileSync('npm', ['run', 'build'], { stdio: 'inherit' });
const targets = [{ abi: 'arm64-v8a', rust: 'aarch64-linux-android', clang: 'aarch64-linux-android26-clang' }];
if (process.argv.includes('--emulator')) targets.push({ abi: 'x86_64', rust: 'x86_64-linux-android', clang: 'x86_64-linux-android26-clang' });
execFileSync('rustup', ['target', 'add', ...targets.map(value => value.rust)], { stdio: 'inherit' });
for (const target of targets) {
  const linker = path.join(bin, target.clang);
  const key = target.rust.replaceAll('-', '_');
  const env = { ...environment, [`CC_${key}`]: linker, [`AR_${key}`]: path.join(bin, 'llvm-ar'), [`CARGO_TARGET_${key.toUpperCase()}_LINKER`]: linker,
    RUSTFLAGS: '-C link-arg=-Wl,-z,max-page-size=16384' };
  execFileSync('cargo', ['build', '--release', '--manifest-path', 'native/core/Cargo.toml', '--target', target.rust, '--target-dir', 'engine/target'], { env, stdio: 'inherit' });
  const destination = path.join(root, 'native/android/app/src/main/jniLibs', target.abi);
  mkdirSync(destination, { recursive: true });
  copyFileSync(path.join(root, 'engine/target', target.rust, 'release/libdrift_native.so'), path.join(destination, 'libdrift_native.so'));
}
const version = '8.13';
const gradleDirectory = path.join(tools, `gradle-${version}`);
if (!existsSync(path.join(gradleDirectory, 'bin/gradle'))) {
  mkdirSync(tools, { recursive: true });
  const archive = path.join(tools, `gradle-${version}.zip`);
  const checksum = `${archive}.sha256`;
  const download = (url, destination) => execFileSync('curl', ['--proto', '=https', '--proto-redir', '=https', '--tlsv1.2', '-fsSL', '--retry', '3', url, '-o', destination], { stdio: 'inherit' });
  download(`https://services.gradle.org/distributions/gradle-${version}-bin.zip`, archive);
  download(`https://services.gradle.org/distributions/gradle-${version}-bin.zip.sha256`, checksum);
  if (createHash('sha256').update(readFileSync(archive)).digest('hex') !== readFileSync(checksum, 'utf8').trim().split(/\s+/)[0]) throw new Error('Gradle archive checksum mismatch');
  execFileSync('unzip', ['-q', '-o', archive, '-d', tools], { stdio: 'inherit' });
}
execFileSync(path.join(gradleDirectory, 'bin/gradle'), ['--no-daemon', '-p', 'native/android', `-PdriftAbis=${targets.map(value => value.abi).join(',')}`, 'assembleDebug'], { env: environment, stdio: 'inherit' });
const output = path.join(root, 'release/android'); mkdirSync(output, { recursive: true });
const apk = path.join(root, 'native/android/app/build/outputs/apk/debug/app-debug.apk');
if (!existsSync(apk)) throw new Error('Gradle did not produce an APK');
const filename = targets.length > 1 ? 'Drift-android-arm64-x86_64-debug.apk' : 'Drift-android-arm64-debug.apk';
copyFileSync(apk, path.join(output, filename));
execFileSync(path.join(sdk, 'build-tools/35.0.0/apksigner'), ['verify', '--verbose', apk], { env: environment, stdio: 'inherit' });
const digest = createHash('sha256').update(readFileSync(apk)).digest('hex');
writeFileSync(path.join(output, `${filename}.sha256`), `${digest}  ${filename}\n`);
writeFileSync(path.join(output, 'SHA256SUMS'), readdirSync(output).filter(name => name.endsWith('.apk')).sort().map(name => `${createHash('sha256').update(readFileSync(path.join(output, name))).digest('hex')}  ${name}\n`).join(''));
writeFileSync(path.join(output, `${filename}.json`), JSON.stringify({ filename, sha256: digest, bytes: readFileSync(apk).length, abis: targets.map(value => value.abi), debugSigned: true, signatureVerified: true, testedOnDevice: false }, null, 2) + '\n');
console.log(`Installable development APK created in ${output}. Debug-signed, not a store release. Included ABIs: ${targets.map(value => value.abi).join(', ')}.`);
// Fail if a build accidentally placed signing material next to distributable files.
if (readdirSync(output).some(name => /\.(jks|keystore|p12|p8)$/.test(name))) throw new Error('Unexpected signing material in release output');
