import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import console from 'node:console';

if (process.platform !== 'darwin') throw new Error('iOS builds require a macOS/Xcode build host.');
const environment = { ...process.env };
if (!environment.DEVELOPER_DIR && existsSync('/Applications/Xcode.app/Contents/Developer')) environment.DEVELOPER_DIR = '/Applications/Xcode.app/Contents/Developer';
try { execFileSync('xcodebuild', ['-version'], { env: environment, stdio: 'pipe' }); }
catch { throw new Error('Full Xcode is not available. On macOS 14 install compatible Xcode 16.2 from Apple, launch it to complete setup, and set DEVELOPER_DIR if needed. Command Line Tools alone cannot build iOS.'); }
const simulator = process.argv.includes('--simulator');
const team = environment.DRIFT_IOS_TEAM_ID;
if (!simulator && !/^[A-Z0-9]{10}$/.test(team ?? '')) throw new Error('A device IPA requires DRIFT_IOS_TEAM_ID and a valid signing certificate/provisioning setup. Use --simulator only for a simulator .app, not a physical-device installer.');
const sdk = simulator ? 'iphonesimulator' : 'iphoneos';
const rust = simulator ? (process.arch === 'arm64' ? 'aarch64-apple-ios-sim' : 'x86_64-apple-ios') : 'aarch64-apple-ios';
environment.SDKROOT = execFileSync('xcrun', ['--sdk', sdk, '--show-sdk-path'], { env: environment, encoding: 'utf8' }).trim();
environment.IPHONEOS_DEPLOYMENT_TARGET = '16.0';
execFileSync('npm', ['run', 'build'], { stdio: 'inherit' });
execFileSync('python3', ['scripts/generate_icons.py'], { stdio: 'inherit' });
execFileSync('rustup', ['target', 'add', rust], { stdio: 'inherit' });
execFileSync('cargo', ['build', '--release', '--manifest-path', 'native/core/Cargo.toml', '--target', rust, '--target-dir', 'engine/target'], { env: environment, stdio: 'inherit' });
const framework = path.resolve('native/ios/Generated/DriftCore.xcframework');
mkdirSync(path.dirname(framework), { recursive: true });
// Only this documented generated build artifact is replaced; source projects remain intact.
if (existsSync(framework)) rmSync(framework, { recursive: true });
execFileSync('xcodebuild', ['-create-xcframework', '-library', path.resolve(`engine/target/${rust}/release/libdrift_native.a`), '-headers', path.resolve('native/core/include'), '-output', framework], { env: environment, stdio: 'inherit' });
const output = path.resolve('release/ios'); mkdirSync(output, { recursive: true });
const common = ['-project', 'native/ios/Drift.xcodeproj', '-scheme', 'Drift', '-configuration', 'Release'];
if (simulator) {
  const derived = path.join(output, 'SimulatorBuild');
  execFileSync('xcodebuild', [...common, '-sdk', sdk, '-destination', 'generic/platform=iOS Simulator', '-derivedDataPath', derived, 'CODE_SIGNING_ALLOWED=NO', 'build'], { env: environment, stdio: 'inherit' });
  const app = path.join(derived, 'Build/Products/Release-iphonesimulator/Drift.app');
  if (!existsSync(app)) throw new Error('Simulator build did not produce Drift.app');
  writeFileSync(path.join(output, 'BUILD.json'), JSON.stringify({ kind: 'ios-simulator-app', path: app, physicalDeviceInstallable: false, testedOnDevice: false }, null, 2));
  console.log('Simulator .app built. No device IPA was produced.');
} else {
  const archive = path.join(output, 'Drift.xcarchive');
  const options = path.join(output, 'ExportOptions.plist');
  const method = environment.DRIFT_IOS_EXPORT_METHOD ?? 'debugging';
  if (!['debugging', 'release-testing', 'app-store-connect'].includes(method)) throw new Error('Invalid DRIFT_IOS_EXPORT_METHOD');
  const provisioning = environment.DRIFT_IOS_ALLOW_PROVISIONING === '1' ? ['-allowProvisioningUpdates'] : [];
  const identifier = environment.DRIFT_IOS_BUNDLE_ID ?? 'dev.drift.transfer';
  if (!/^[A-Za-z0-9.-]+$/.test(identifier)) throw new Error('Invalid bundle identifier');
  execFileSync('xcodebuild', [...common, '-destination', 'generic/platform=iOS', '-archivePath', archive, `DEVELOPMENT_TEAM=${team}`, `PRODUCT_BUNDLE_IDENTIFIER=${identifier}`, ...provisioning, 'archive'], { env: environment, stdio: 'inherit' });
  writeFileSync(options, `<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>method</key><string>${method}</string><key>teamID</key><string>${team}</string><key>signingStyle</key><string>automatic</string></dict></plist>`);
  execFileSync('xcodebuild', ['-exportArchive', '-archivePath', archive, '-exportPath', output, '-exportOptionsPlist', options, ...provisioning], { env: environment, stdio: 'inherit' });
  if (!existsSync(path.join(output, 'Drift.ipa'))) throw new Error('Xcode did not export an IPA. Check provisioning; an archive alone is not installable.');
  writeFileSync(path.join(output, 'BUILD.json'), JSON.stringify({ kind: 'signed-ios-ipa', method, physicalDeviceInstallable: method !== 'app-store-connect', testedOnDevice: false }, null, 2));
}
