# Native release work

The unused-code audit was performed before native additions; see
UNUSED-CODE-AUDIT.md. This repository is local at ~/Documents/repos/drift.

## Architecture

Retain the tested Electron desktop shell. Package a DMG on macOS and NSIS installer
on Windows using that platform's Rust build. Native mobile apps embed the same
Rust engine and bundled React assets, with thin Kotlin/Android WebView and
SwiftUI/WKWebView hosts. The mobile engine runs inside the app process, not as a
background subprocess or a hosted website. Its UI binds only to loopback and
uses the existing random control capability. Pairing between native devices uses
the existing certificate-bound high-entropy proof, not manually installed roots.

A small C ABI/JNI boundary starts/stops the engine and exposes only verified,
completed received files for the OS save picker. Source file selection uses the
OS document picker/WebView upload flow. A receiving phone must be able to export
files from its sandbox; simply showing an inaccessible filesystem path is not a
complete receiving workflow. No automatic opening/execution or arbitrary-path
JavaScript bridge is permitted. Mobile background operation remains OS-limited.

## Produced development artifacts and remaining gates

* Intel macOS DMG produced at `release/Drift-0.2.0-mac-x64.dmg`;
  `hdiutil verify` passed. Unsigned and not notarized.
* Apple Silicon DMG produced at `release/Drift-0.2.0-mac-arm64.dmg`;
  disk-image integrity, mounted ARM64 binaries and deep ad-hoc signature passed.
  Not Developer ID signed/notarized; runtime testing requires an Apple Silicon Mac.
* Windows x64 NSIS installer produced at
  `release/windows/Drift-0.2.0-windows-x64.exe` using cross-compilation.
  Unsigned; not installed or runtime-tested on Windows.
* Android ARM64 and ARM64/x86_64 debug APKs produced in `release/android/`;
  APK v2 signature verification passed. Physical-device testing remains pending.
  The Android 35 x86_64 emulator test passed for native startup, authenticated
  TLS receive, exact bytes and complete system-picker save to Downloads with
  external-byte verification. Physical LAN discovery and physical-phone testing
  remain separate gates.

* Xcode is now authorized but not installed. This macOS 14.8.9 host supports
  Xcode 16.2 according to Apple's compatibility table. Newer Xcode releases need
  a newer macOS build host; no OS upgrade is authorized. Xcode downloads/login
  and Apple signing setup require the owner. Do not call a simulator .app or
  unsigned archive a device-installable IPA.
* User authorized installing the JDK/Android SDK and accepting SDK license terms.
  The scoped setup script verifies downloaded archives and avoids global shell
  changes. Build-tool caches and signing material are excluded from Git.
* Windows runtime testing requires Windows. Native-runner CI is also prepared;
  preparing CI is not the same as running it. No hosted repository has been
  authorized and no workflows may be represented as successfully executed.
* Physical iOS installs need suitable provisioning/signing. App Store/TestFlight
  distribution has additional Apple requirements. These developer-side accounts
  must not become mandatory accounts for file-transfer users.

Source projects, compiled artifacts, installation tests and physical transfer
tests are tracked separately. Do not mark a platform complete because a project
folder or build workflow exists.
