import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import process from 'node:process';

test('desktop packaging rejects unsupported architecture before compilation', () => {
  const result = spawnSync(process.execPath, ['scripts/package-desktop.mjs', 'dmg', 'riscv64'], { encoding: 'utf8' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Desktop architecture must be x64 or arm64/);
});

test('JNI exports match every Android native method', () => {
  const kotlin = readFileSync('native/android/app/src/main/java/dev/drift/app/NativeBridge.kt', 'utf8');
  const rust = readFileSync('native/core/src/lib.rs', 'utf8');
  const methods = [...kotlin.matchAll(/external fun (\w+)\(/g)].map(match => match[1]).sort();
  const exports = [...rust.matchAll(/fn Java_dev_drift_app_NativeBridge_(\w+)\(/g)].map(match => match[1]).sort();
  assert.deepEqual(exports, methods);
  assert.match(kotlin, /System\.loadLibrary\("drift_native"\)/);
});

test('native startup backgrounds stay black and embedded UI changes trigger rebuilds', () => {
  const desktop = readFileSync('desktop/main.cjs', 'utf8');
  const android = readFileSync('native/android/app/src/main/java/dev/drift/app/MainActivity.kt', 'utf8');
  const colors = readFileSync('native/android/app/src/main/res/values/colors.xml', 'utf8');
  const theme = readFileSync('native/android/app/src/main/res/values/themes.xml', 'utf8');
  const manifest = readFileSync('native/android/app/src/main/AndroidManifest.xml', 'utf8');
  const ios = readFileSync('native/ios/Sources/DriftApp.swift', 'utf8');
  const build = readFileSync('native/core/build.rs', 'utf8');
  assert.match(desktop, /backgroundColor: '#000000'/);
  assert.match(android, /view\.setBackgroundColor\(android\.graphics\.Color\.BLACK\)/);
  assert.match(colors, /#000000/);
  assert.match(manifest, /android:theme="@style\/DriftTheme"/);
  assert.match(theme, /android:windowBackground">@color\/drift_background/);
  assert.doesNotMatch(theme, /Theme\.Material\.Light/);
  assert.match(ios, /preferredColorScheme\(\.dark\)/);
  assert.match(ios, /view\.backgroundColor = \.black/);
  assert.match(build, /cargo:rerun-if-changed=\.\.\/\.\.\/dist\/ui/);
});

test('Android passes only OS-discovered Wi-Fi or Ethernet addresses to the native engine', () => {
  const host = readFileSync('native/android/app/src/main/java/dev/drift/app/MainActivity.kt', 'utf8');
  const bridge = readFileSync('native/android/app/src/main/java/dev/drift/app/NativeBridge.kt', 'utf8');
  const rust = readFileSync('native/core/src/lib.rs', 'utf8');
  assert.match(host, /getLinkProperties/);
  assert.match(host, /!capabilities\.hasTransport\(NetworkCapabilities\.TRANSPORT_VPN\)/);
  assert.match(host, /TRANSPORT_WIFI/);
  assert.match(host, /TRANSPORT_ETHERNET/);
  assert.match(host, /isSiteLocalAddress \|\| it\.isLinkLocalAddress/);
  assert.match(bridge, /start\(directory: String, name: String, localHost: String\)/);
  assert.match(rust, /supplied_lan_host\(&host\)\?/);
});

test('native mobile shells do not bypass remote certificate validation', () => {
  const android = readFileSync('native/android/app/src/main/java/dev/drift/app/MainActivity.kt', 'utf8');
  const ios = readFileSync('native/ios/Sources/DriftApp.swift', 'utf8');
  const config = readFileSync('native/android/app/src/main/res/xml/network_security_config.xml', 'utf8');
  assert.doesNotMatch(android, /onReceivedSslError|handler\.proceed\(/);
  assert.doesNotMatch(ios, /useCredential|NSURLAuthenticationMethodServerTrust/);
  assert.match(config, /base-config cleartextTrafficPermitted="false"/);
  assert.match(config, /<domain>127\.0\.0\.1<\/domain>/);
});

test('iOS project/plist structure and RGB app icon are valid (not an SDK build)', { skip: process.platform !== 'darwin' }, () => {
  const plist = file => JSON.parse(execFileSync('plutil', ['-convert', 'json', '-o', '-', file], { encoding: 'utf8' }));
  const project = plist('native/ios/Drift.xcodeproj/project.pbxproj');
  assert.ok(project.objects[project.rootObject]);
  for (const id of Object.keys(project.objects)) assert.match(id, /^[A-F0-9]{24}$/);
  assert.equal(project.objects.A00000000000000000000004.productType, 'com.apple.product-type.application');
  const info = plist('native/ios/Info.plist');
  assert.equal(info.NSAppTransportSecurity.NSAllowsLocalNetworking, true);
  assert.notEqual(info.NSAppTransportSecurity.NSAllowsArbitraryLoads, true);
  assert.ok(info.NSBonjourServices.includes('_drift._tcp'));
  assert.ok(existsSync('native/ios/Sources/Drift-Bridging-Header.h'));
  const png = readFileSync('native/ios/Assets.xcassets/AppIcon.appiconset/AppIcon.png');
  assert.equal(png.readUInt32BE(16), 1024); assert.equal(png.readUInt32BE(20), 1024);
  assert.equal(png[25], 2, 'iOS icon must be RGB without an alpha channel');
});

test('signing materials and generated outputs are ignored, but the iOS project is tracked source', () => {
  const ignored = file => spawnSync('git', ['check-ignore', '--no-index', '-q', file]).status === 0;
  for (const file of ['local.p12', 'local.mobileprovision', 'local.keystore', '.toolchains/cache.zip', 'release/test.apk', 'native/android/app/src/main/jniLibs/arm64-v8a/libdrift_native.so']) assert.ok(ignored(file), file);
  assert.equal(ignored('native/ios/Drift.xcodeproj/project.pbxproj'), false);
});
