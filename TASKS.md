# Delivery checklist

- [x] Inspect existing project and run its smoke tests (passed).
- [x] Select architecture, protocol and threat model.
- [x] Build and test certificate-bound account-free pairing (120-bit temporary code).
- [x] Implement explicit device and incoming-manifest approval.
- [x] Stream bounded chunks over pinned TLS; verify SHA-256.
- [x] Persist offsets; test reconnection/re-pairing after receiver restart; preserve folder paths.
- [x] Implement pause/cancel/retry controls; test cancellation propagation and concurrent receives.
- [x] Integrate mDNS; test advertisement, resolution and removal on this host.
- [x] Generate expiring QR invitations and support pasting invitation contents.
- [ ] Implement camera QR scanning and reviewed short-code PAKE pairing.
- [x] Build React interface with themes, responsive layout and keyboard controls.
- [x] Browser E2E: pair two actual engine processes, approve and transfer, check bytes on disk.
- [x] Type checks, frontend build/lint, Rust tests and Clippy run successfully.
- [x] Run a release-mode loopback benchmark; document limitations instead of claiming AirDrop superiority.
- [x] Build/package an Electron macOS Intel app without Xcode.
- [x] Native user-driven file picker, narrow IPC and direct source reads (no staging copy).
- [x] Eliminate staging for OS-backed desktop drag-and-drop through isolated native import.
- [x] HTTPS Android/iPhone browser companion with manual CA/profile trust, one-use invitations and both-device approval.
- [x] Phone upload and verified OPFS-worker download; no desktop administration access.
- [ ] Validate certificate installation, home-screen launch and background limits on physical phones.
- [ ] Exercise two physical devices and operating systems.
- [ ] Remote ICE/STUN/TURN and signaling with Docker deployment.
- [ ] Native mobile clients and background/file-access integrations.
- [ ] Persistent trusted identities and OS credential-store integration.
- [ ] Packaging, signing, notarization, update security and external security review.

## Standalone native development builds

- [x] Preserve original work; copy Drift into local `~/repos/drift` Git repository.
- [x] Audit unused code before native work; remove redundant staging parameter.
- [x] Add shared Rust C ABI/JNI lifecycle and restricted received-file export tests.
- [x] Compile Kotlin Android host and embedded Rust engine; verify debug APK signature.
- [x] Produce unsigned Intel macOS DMG; verify disk-image checksum.
- [x] Produce Apple Silicon ARM64 DMG; verify mounted binaries and deep ad-hoc signature.
- [x] Cross-compile Windows x64 engine and unsigned NSIS installer.
- [x] Prepare SwiftUI/WKWebView source, Xcode project and explicit signing build gates.
- [x] Android emulator: native boot, TLS pairing/receive, byte verification and export picker.
- [x] Android emulator external save-copy completion and saved-byte verification.
- [ ] Android physical-device testing and physical LAN discovery.
- [ ] Build/test iOS with full Xcode and suitable device signing (currently unavailable).
- [ ] Validate Windows installer/runtime and Apple Silicon launch/transfers on matching hosts.
- [ ] Production signing, notarization, secure updates and external review.
- [x] Bounded source-read/upload pipeline, shared retry buffers and recovery tests.
- [x] Negotiate bounded durable windows with legacy fallback; enforce approval, size limits and ownership.
- [x] Prefer local hotspot/Wi-Fi interfaces over cellular/VPN addresses in invitations; test selection rules.
- [ ] Measure physical hotspot throughput against Blip on identical hardware/files.

See [native/README.md](native/README.md) for commands and artifact limitations.

### Black-theme rebuild validation (2026-10-18)

All five available development artifacts were rebuilt with the true-black,
card-free UI: Intel/ARM64 DMGs, Windows x64 installer, and ARM64/dual-ABI debug
APKs. Packaged UI parity, architectures, image integrity and APK signatures/page
alignment passed. Intel packaged startup, seven UI E2Es (one WebKit test explicitly
skipped), 32 engine tests in both debug/release, three native-core tests, mDNS and
loopback streaming passed. Android's restricted interface enumeration was fixed
using OS Wi-Fi/Ethernet LinkProperties with private-address validation; repeat
emulator receive/export checks passed. npm/Rust advisory audits found no known
vulnerabilities; RustSec reports the transitive `rustls-pemfile` maintenance warning.

Jev assessed the test plan, execution evidence and visual brief; its recommendation
is private development testing only. Model judgments do not replace executed
tests. iOS remains blocked by missing full Xcode; Windows, Apple Silicon and
physical-phone runtime testing, production signing and external review remain
open. Current logs and assessments are in `release/validation-black/`.

## Scope note

The initial core passed 14 integration/security tests in debug and release modes,
two browser E2Es, an opt-in mDNS lifecycle test and five loopback benchmarks.
The desktop/phone iteration adds five tests for strict phone TLS trust,
enrollment/revocation, scoped uploads/downloads, native import authority,
zero-byte verification and pending-manifest quotas. All 19 Rust tests passed.
A real Electron/Chromium E2E transfers both ways and checks saved bytes without
desktop staging. The packaged Mac app's bundled-engine startup/shutdown smoke
test passed. The browser suite passed 4 tests with 1 explicitly blocked WebKit
case skipped on this host. TypeScript,
ESLint and Rust Clippy passed.
Benchmark/mDNS are opt-in.

The installed frozen WebKit on macOS 14 cannot create even a blank page. That
phone test is explicitly blocked/skipped here, not counted as passing. See the
15-second probe and opt-in command in PHONE-SETUP.md. Physical iPhone/Android
certificate installation and home-screen operation are still unverified.

This is a working LAN preview, not the complete requested product or a production
security certification. No physical-device, Windows, Linux, Apple Silicon,
Wi-Fi-throughput or AirDrop comparison has been performed. Directory drag-and-drop
code uses Chromium/WebKit entry APIs; the folder picker and preserved paths are
covered by browser/core tests. Empty directories are not transferred. Browser
staging and retained recovery files consume additional disk space.
