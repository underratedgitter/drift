# Drift release gates

## Current decision

**Private development preview only. No remaining native/device/signing gate is
closed by this runbook or an unexecuted CI configuration.**

The owner currently has no additional machines, physical phones, full Xcode or
signing setup available for these checks. The latest executed results are in
[the validation report](release/validation-2026-10-19/FINAL-REPORT.md). That report
records an Android emulator startup failure followed by two successful runs;
do not rewrite that as three passes.

This document prepares the work; it does not publish a repository, dispatch CI,
buy runner time, install on a personal device, import signing keys or enable an
automatic updater. Repository publication and hosted-runner usage need the
owner's explicit approval. Private-repository runner minutes can incur charges.

All commands below run from the repository root with Node 24 and a suitable Rust
toolchain. Use generated test files, never sensitive personal files.

## Native desktop validation

### Prepared manual CI

`.github/workflows/native-builds.yml` is manual (`workflow_dispatch`) and has
read-only repository permissions. After the owner authorizes a hosted repository
and runner usage, choose `macos`, `windows`, or `all` in Actions.

- macOS runs on **macos-15-intel/x64** and **macos-15/arm64**, checks the actual
  Node architecture, builds that architecture, and runs core/native tests, UI
  E2Es, packaged startup/shutdown and bundled-engine transfer checks.
- Windows runs on **windows-2025/x64**, builds natively, and exercises the native
  **unpacked** app. This does **not** prove the NSIS installer was installed or
  uninstalled successfully.
- macOS explicitly enables the WebKit phone E2E instead of allowing the known
  local-host skip. Playwright WebKit is still not physical iPhone Safari.
- JSON UI results and host/architecture metadata are uploaded as runtime
  evidence with seven-day retention. A host metadata file alone is not a pass:
  inspect job conclusions, individual test results, skips and artifact hashes.
- Android and iOS-simulator jobs remain **build-only**. The simulator output is
  not a device IPA, physical phone, background test or signing certification.

GitHub documents the runner labels and architectures in its
[hosted runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Runner availability and billing must be checked before dispatch. None of these
new native runner jobs has been executed as part of preparing this document.

### Commands on a matching machine

```sh
npm ci
npm run test:build-config
npm run test:release-tools
npm test
npm run test:native
```

Then build on that OS/architecture:

```sh
# On either Intel or Apple Silicon macOS; builds the host architecture.
npm run mac:installer

# On Windows x64, not on this Mac.
npm run windows:installer
```

Install Playwright's Chromium/WebKit dependencies, then run the browser/native
checks with `npm run test:ui`. The helper chooses `release/mac/Drift.app` on
Intel, `release/mac-arm64/Drift.app` on ARM64, and
`release/win-unpacked/Drift.exe` on native Windows builds.

For packaged transfers, set `DRIFT_TEST_PACKAGED_TRANSFERS=1` and run:

```sh
npx playwright test tests/ui/desktop-phone.spec.ts --grep 'packaged app and Chromium'
```

macOS/Linux environment syntax is `export NAME=value`; PowerShell is
`$env:NAME = 'value'`. If testing an **installed** app or the Mac-created Windows
cross-build, set `DRIFT_TEST_PACKAGED_EXECUTABLE` to its **absolute executable
path**. Do not silently substitute the development engine: the packaged mode
removes `DRIFT_ENGINE_PATH`, requires `app.isPackaged`, and checks Electron's
architecture against the native test host.

### Installer acceptance remains manual

On a disposable Windows VM, explicitly approve and install the exact NSIS EXE,
including a nondefault destination; test shortcuts, launch, transfer, close,
relaunch, and uninstall. Verify uninstall does not delete separately saved
received files. Record SmartScreen/firewall prompts; do not disable protection
to claim a production pass. Test a standard user, not only an administrator VM.

On a matching Mac, mount the exact DMG, copy the app through the normal user
workflow and test the copied app, including local-network permission prompts.
For production acceptance, use a quarantined download of the signed/notarized
artifact; do not bypass Gatekeeper. Source builds and an ad-hoc signature are
not substitutes for trusted distribution.

## Physical mobile and Wi-Fi validation

Create a deterministic fixture without personal data:

```sh
node scripts/transfer-fixture.mjs create
```

It creates a new directory under `release/manual-fixtures/`, prints its paths,
and refuses to overwrite an existing destination. Send its **files/** directory,
not the adjacent manifest. The 12 expected files include an empty file, spaces,
Unicode, nested paths, eight small files and a 17-MiB-plus file that crosses a
16-MiB durable window.

After transfer/export, compare received files against the original manifest:

```sh
node scripts/transfer-fixture.mjs verify <manifest.json> <received-root>
```

Replace angle-bracket arguments with real paths and quote paths with spaces.
Select the received root whose relative paths match the manifest (for a folder
send this may be the `files` subdirectory of the received transfer). A mismatch,
missing empty file, bad size or bad SHA-256 returns a nonzero exit code. The
verifier checks expected files; it is not a security audit of unrelated files.

For individual mobile exports that do not recreate external folders, verify
each chosen saved file's SHA-256 against its manifest entry using `shasum -a 256`
or PowerShell `Get-FileHash -Algorithm SHA256`. A private sandbox path or a
"complete" label alone is not successful OS save/export.

Run and record both directions for each available pair:

| Pair / network | Required result | Current status |
|---|---|---|
| Mac ↔ Windows on the same physical Wi-Fi | Pair, both-device approval, transfer, saved-byte equality | NOT RUN |
| Mac ↔ native Android on Wi-Fi | Same, plus actual Android system pickers | NOT RUN |
| Mac ↔ native iPhone on Wi-Fi | Same, with a signed device build and iOS Files export | NOT RUN |
| Laptop ↔ Android hotspot | Usable local address, discovery/manual pairing, verified transfer | NOT RUN |
| Laptop ↔ iPhone hotspot | Same; no cellular/VPN address advertised as the LAN address | NOT RUN |
| Android ↔ iPhone | Native-to-native pairing and verified files in both directions | NOT RUN |

Record OS versions, Wi-Fi/AP setup, app hashes, addresses with sensitive details
redacted, discovery versus manual pairing, and whether client isolation blocks
the link. An isolated AP is an environment limitation, not a passed network test.
No internet exposure, router forwarding or cellular transfer is required.

Native app pairing does not require installing the phone-browser CA profile.
Phone-browser certificate installation is a separate acceptance test; never
bypass TLS warnings or install an unverified replacement root.

## Pickers, background operation and interruptions

Mobile is currently **foreground-only**. Do not claim OS background privileges.
Test interruption correctness, not an unsupported promise of uninterrupted
background throughput.

1. Use the actual Android/iOS **outgoing** system picker to select empty, Unicode,
   multichunk and multiple files. Cancel it once: no unintended send may occur.
2. Receive a verified file, choose the actual save destination, confirm the file
   exists outside the app sandbox, and hash it. Cancel export once: do not report
   a successful save. Individual-file export may flatten external folders as
   documented; the private received structure and contents must remain correct.
3. During a large transfer, switch apps, lock the screen and return. OS suspension
   is allowed; false completion, silently corrupt data and unapproved publication
   are not. Record whether manual resume/reselection is necessary.
4. Interrupt Wi-Fi, reconnect, and retry using the documented controls. After a
   process restart, generate a fresh invitation/re-pair if required, reselect
   the same source files, and verify the final saved bytes. Do not infer automatic
   trusted-device reconnect: persistent trust is not implemented.
5. Test explicit pause/cancel, receiver rejection, app termination, and simultaneous
   transfers. Leave existing received files intact. Perform low-storage tests
   only on a controlled disposable device; do not fill a personal phone's disk.
6. Repeat exports and transfers after relaunch. Check actionable failure messages,
   no incomplete file presented as verified, and no overwrite of existing exports
   without the OS/user's explicit choice.

Each case remains NOT RUN on physical phones until evidence exists. Core restart
tests and emulator export tests do not close these device-specific cases.

## Safari and WebKit

The local macOS 14 WebKit probe times out creating a blank page. Recheck before
claiming compatibility:

```sh
node scripts/probe-webkit.mjs
```

On a supported Mac where that passes, install Playwright WebKit, set
`DRIFT_TEST_WEBKIT=1`, then run:

```sh
npx playwright test tests/ui/desktop-phone.spec.ts --grep 'WebKit phone'
```

Require an actual passing test, not a skip. Playwright uses an isolated test-only
TLS override; this does not validate installing/trusting the certificate on a real
iPhone. Test current physical iPhone Safari separately: correct trust setup,
enrollment/approval, upload, OPFS download, verification, Save to Files, reopening
and foreground/background limits. Also test the native WKWebView app separately.

## Signing and notarization

These are owner-controlled production tasks; the current packagers deliberately
produce unsigned/ad-hoc/debug development artifacts. Supplying credentials alone
does not make those scripts a production signing pipeline.

- **macOS:** obtain/configure Developer ID locally, review entitlements/hardened
  runtime, sign the nested engine/Electron bundle correctly, notarize the final
  distribution with Apple's service and staple it. Verify with `codesign`,
  `spctl`, and `xcrun stapler validate`, then run the quarantined-download test.
- **Windows:** select a trusted code-signing method, protect the key/service
  access, sign and timestamp the appropriate binaries and final installer, verify
  signatures on a clean Windows system, and record reputation/SmartScreen results.
- **Android:** create a protected release/upload key, configure a separate release
  signing path and version policy, verify the resulting APK/AAB and any store
  requirements. Never publish the debug APK as a production/store build.
- **iOS:** full Xcode and a compatible SDK/runner, Apple team/certificate/profile
  and the intended export method are required. Use `.env.example` as a variable
  reference; it is not automatically loaded. Validate the exported device IPA
  and test a physical device. A simulator `.app` is not an IPA.

Use Keychain/OS credential storage or an approved secret manager. Do not paste
passwords or private keys into chat, commit them, attach them to test evidence,
or upload them as workflow artifacts. Key recovery, rotation and revocation need
an owner-approved procedure. No signing or store upload is automated here.

## Secure updates and external review

### Updates: design and signing prerequisites first

No automatic updater is implemented or enabled by this work. Keep development
distribution manual. Before implementing an updater, agree on the update host,
signed release channel, platform installer format, supported versions and who
controls/protects the release keys. At minimum require:

- authenticated metadata and payloads with trusted keys anchored in the installed
  app; a checksum served beside an unsigned download is not authentication;
- correct platform/architecture selection, version/rollback policy, expiration,
  key rotation/revocation and fail-closed handling of tampered metadata/payloads;
- no unattended execution of arbitrary downloaded code and no renderer-controlled
  update paths or privileged installer arguments;
- transactional/interruption-safe install and recovery that preserve received
  files, user settings and private transfer state;
- tests for invalid signatures, altered bytes, obsolete releases, wrong architecture,
  interrupted downloads/installations and a compromised update endpoint.

These are requirements, not a claim of an implemented or reviewed update system.

### Independent security review

The owner must engage an independent qualified reviewer. AI advice, local tests
and dependency audits are not that review. Provide the exact candidate hashes,
`ARCHITECTURE.md`, `SECURITY.md`, source and sanitized test evidence. Scope includes
the custom high-entropy pairing protocol (do not shorten its code), certificate
binding/replay/approval, capability/origin isolation, IPC/JNI/WKWebView boundaries,
path safety/publication/resume semantics, resource exhaustion, OS file permissions,
credential storage, signing and the proposed update trust model.

Require written findings, severity/ownership, fixes and retest evidence tied to
the final candidate. Current `rustls-pemfile` maintenance warnings should be tracked;
zero known dependency vulnerabilities does not prove protocol or app security.

## Required evidence

Use a fresh record per gate, device, candidate and attempt. Never replace a failed
attempt with an unexplained passing rerun. The following is a **template**, not a
test result:

```json
{
  "gate": "physical-android-outgoing-picker",
  "status": "NOT_RUN",
  "candidateSha256": null,
  "sourceRevision": null,
  "deviceAndArchitecture": null,
  "osVersion": null,
  "networkSetup": null,
  "stepsAndExpectedResults": [],
  "observedResults": [],
  "savedFileHashes": [],
  "limitations": [],
  "testerAndTimestamp": null,
  "sanitizedEvidencePaths": []
}
```

Use PASS / FAIL / BLOCKED / NOT_RUN explicitly. Evidence should show the actual
installer hash, OS/architecture, executed test cases, saved-byte comparisons and
each relevant prompt. Redact pairing codes, invitation URLs, authorization headers,
private keys and personal file content. Store local records under `release/` in
this repository; upload only sanitized evidence with owner approval.

**Exit criterion:** only close a gate after its matching environment and exact
candidate pass the required cases. Until then, the release remains a private
development preview regardless of how many other platforms compile.
