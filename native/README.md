# Native apps and installers

The Kotlin and SwiftUI clients embed the Rust engine and bundled React UI.
They are not wrappers around a remote website or the phone-browser gateway.
Native pairing uses the same temporary-code/certificate-bound authentication as
desktop pairing, without installing a phone CA profile or creating an account.
Keep native mobile apps foregrounded; native background privileges are not claimed.

The optimized core uses direct LAN TLS, negotiated 16 MiB durable windows with
an 8 MiB legacy fallback, one-window source read-ahead, and shared retry buffers.
Application payload buffering is capped at 32 MiB on the sender; library/OS
buffers are additional. Desktop native pickers AND OS-backed drag-and-drop
bypass staging. Mobile WebView uploads still stage into private app storage.
See PERFORMANCE.md for the hotspot test protocol and measured limitations.

## Outputs

| Platform | Command | Output | Limit |
|---|---|---|---|
| macOS | `npm run mac:installer` | `release/*.dmg` | Unsigned local development build, host architecture |
| Apple Silicon macOS | `npm run mac:installer:arm64` | `release/Drift-*-mac-arm64.dmg` | ARM64 engine and Electron; ad-hoc signed, not notarized |
| Windows | `npm run windows:installer` on Windows | `release/*.exe` | Unsigned NSIS installer, native runner required |
| Windows cross-build | `npm run windows:cross` on macOS | `release/windows/*.exe` | Cross-compilation does not establish Windows runtime correctness |
| Android | `npm run android:package` | `release/android/Drift-android-arm64-debug.apk` | ARM64, Android 8+, debug-signed development build |
| iOS device | `npm run ios:package` | `release/ios/Drift.ipa` only after successful export | Requires full Xcode, team, certificate and provisioning |
| iOS simulator | `npm run ios:simulator` | simulator `Drift.app` | Not an IPA and not installable on physical phones |

Only files actually produced by successful build commands count as artifacts.
There are no placeholder EXE/IPA files. SHA-256 output accompanies Android and
desktop installers. Installation and physical transfer testing are separate gates.

The Apple Silicon command supports cross-compilation from an Intel Mac using the
Command Line Tools macOS SDK. It downloads the pinned ARM64 Electron distribution
instead of copying the Intel host runtime, and checks the engine, app executable
and Electron framework architectures. Intel artifacts and architecture-specific
build reports remain separate. Ad-hoc signing supplies local code integrity only,
not a trusted Developer ID signature or notarization. Runtime and transfer testing
on an Apple Silicon Mac are still required.

## Android

The owner authorized JDK/SDK installation and Google's SDK terms. Tools are
scoped under `.toolchains/` (ignored by Git); shell startup files are unchanged.
Bootstrap, if needed:

```sh
python3 scripts/setup-android-tools.py --accept-android-license
npm run android:package
```

The debug APK is intended for local development installation, not Google Play.
Use Android's normal explicit install confirmation for your own build. Native
libraries are linked for 16-KiB page alignment. Use `--emulator` on the build
script to include x86_64 too. `npm run test:android` passed on an isolated AOSP
Android 35 x86_64 emulator: JNI engine startup, black card-free UI,
authenticated TLS pairing/approval, multi-chunk receive, exact byte verification,
native export-picker opening and completed save to Downloads with byte equality.
The test uses ADB forwarding for emulator NAT; it does not establish real Wi-Fi
discovery or physical-phone performance. Native picker automation also requires
`npx playwright install android` on the test host.
Results and tested APK digest are in `release/android/EMULATOR-TEST.json`.
The app requests Internet/network/multicast permissions, not broad filesystem
access. File input uses the system picker; verified received files are exported
through **History → Save received files**, then the system save sheet. Folder
selection/export is restricted by mobile APIs; individual-file export does not
recreate arbitrary external directory trees. Incoming private folder structure
is preserved by the shared engine.

Android's sandbox denies the desktop hard-link publication optimization. Android
instead copies verified data to an unpublished private tree, flushes it, verifies
SHA-256 again and atomically renames the tree. Incomplete or unverified copies are
never presented as received files. This needs extra local disk space; no Android
speed comparison with AirDrop is claimed.

## iOS/Xcode

Open `native/ios/Drift.xcodeproj` after building its generated Rust XCFramework.
The checked-in project is not ignored; only generated frameworks, user state and
signing material are excluded. Icons are original generated RGB assets.

Full Xcode is currently absent on the development Mac. Apple lists Xcode 16.2
as compatible with macOS 14.8.9; download it through your Apple developer account
and finish its first-launch setup, or use a newer macOS runner. This project does
not upgrade the host OS or request your Apple password in chat.

Build a simulator app without distribution signing using `npm run ios:simulator`.
For a physical device, set the variables documented in `.env.example` and install
your signing certificate/profile in Xcode/Keychain. Automatic provisioning is
opt-in. App Store/TestFlight requirements may need a newer SDK/runner than this
Mac can host. A simulator archive is never relabeled as a device IPA.

The Swift host restricts navigation and native messages to the app's own loopback
origin, confines exports to completed received-file IDs, and presents Files' native
save sheet. Its data directory is excluded from automatic backup. Local-network
and file/photo prompts remain user-controlled. No broad ATS bypass is used.

## Windows and CI

The macOS cross-build uses a checksum-pinned LLVM/MinGW toolchain and Rust's
Windows GNU/LLVM target. It inspects DLL imports before bundling; installation
and transfer tests still need Windows. Cross-builds disable Windows resource
signing while retaining icon/version resource editing. No signature or Windows
runtime test is claimed.

The manual `.github/workflows/native-builds.yml` provides native platform runners
if the owner later publishes this repository. It has not been run, and no online
repository or releases have been created. It uploads development artifacts only;
it does not publish a public release or consume signing secrets automatically.
It now prepares separate Intel/ARM64 macOS runtime checks and Windows x64
unpacked-app checks, including bundled-engine transfers. Windows installer
installation and physical-device/network behavior remain separate gates. See
[RELEASE-GATES.md](../RELEASE-GATES.md) for the runbook, fixture/hash verifier,
signing prerequisites and independent-review requirements. Preparing CI does not
authorize publishing a repository, dispatching runners or incurring charges.

## Checks

```sh
npm run audit:unused
npm run typecheck
npm run lint
npm test
npm run test:native
```

Shared Rust lifecycle/asset authorization/export-boundary tests run on this host.
Cross-target compilation, APK signature inspection and plist syntax validation
do not replace native UI, device discovery, interruption and transfer tests.
